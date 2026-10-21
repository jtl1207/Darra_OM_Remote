// tag 云端同步：乐观更新 + 失败回滚 + 离线重试 + SSE/拉取回声去重
import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:get/get.dart';

import 'om_api.dart';
import 'om_auth.dart';

/// 待重试的 tag 写入（离线/网络错时进内存队列）
class _TagWrite {
  final String uid;
  final List<String> tags;
  int attempts = 0;
  _TagWrite(this.uid, this.tags);
}

/// 本机写回指纹，用于吞掉 SSE / 轮询的同值回声
class _EchoStamp {
  final String fingerprint;
  final int revision;
  final DateTime at;
  _EchoStamp(this.fingerprint, this.revision, this.at);
}

/// tag 内存态唯一真相：设备 uid -> tag 列表（UI 分组筛选用 allTags）
class TagSync {
  TagSync._();
  static final TagSync instance = TagSync._();

  final RxMap<String, List<String>> tagsByUid = <String, List<String>>{}.obs;
  final RxList<String> allTags = <String>[].obs;

  /// SSE device-state 驱动的在线状态回调，供 UI 层接
  void Function(String uid, bool online)? onDeviceState;

  final OmApi _api = OmApi();
  final Map<String, _TagWrite> _retryQueue = {};
  final Map<String, _EchoStamp> _echo = {};
  final Map<String, int> _revision = {};
  StreamSubscription<OmSseEvent>? _sseSub;
  Timer? _retryTimer;
  String _sseToken = '';

  /// 去空、去重、保序
  List<String> _normalize(List<String> tags) {
    final out = <String>[];
    for (final t in tags) {
      final s = t.trim();
      if (s.isNotEmpty && !out.contains(s)) out.add(s);
    }
    return out;
  }

  /// 指纹与顺序无关，避免同集合不同顺序被误判为不同值
  String _fingerprint(List<String> tags) {
    final list = _normalize(tags)..sort();
    return list.join(' ');
  }

  /// 全量 tag 词表（UI 分组筛选用）
  void _rebuildAllTags() {
    final set = <String>{};
    for (final list in tagsByUid.values) {
      set.addAll(list);
    }
    allTags.assignAll(set.toList()..sort());
  }

  /// 列表拉取后吸收 tags（缺 tags 字段时为空列表，保持已知态不清空）
  void absorbFromDevices(List<OmDeviceData> devices) {
    for (final d in devices) {
      _applyRemote(d.uid, d.tags, _parseRevision(d.raw['revision']), 'devices');
    }
  }

  static int? _parseRevision(dynamic raw) =>
      raw is int ? raw : int.tryParse('${raw ?? ''}');

  /// 远端（SSE/拉取）单台 tag 落地：revision 比大小 + 回声去重 + 同值跳过
  void _applyRemote(String uid, List<String> tags, int? rev, String source) {
    if (uid.isEmpty) return;
    if (rev != null) {
      final known = _revision[uid];
      if (known != null && rev <= known) {
        debugPrint('[TagSync] 跳过旧 revision uid=$uid rev=$rev known=$known '
            'source=$source');
        return;
      }
      _revision[uid] = rev;
    }
    if (_isEcho(uid, tags)) {
      debugPrint('[TagSync] 吞掉本机回声 uid=$uid source=$source');
      return;
    }
    final old = tagsByUid[uid];
    if (old != null && _fingerprint(old) == _fingerprint(tags)) return;
    tagsByUid[uid] = List<String>.from(tags);
    _rebuildAllTags();
    debugPrint('[TagSync] 远端 tag 落地 uid=$uid tags=$tags source=$source');
  }
  /// 本机刚写回的同值，在短窗口内视为回声不重复应用
  bool _isEcho(String uid, List<String> tags) {
    final stamp = _echo[uid];
    if (stamp == null) return false;
    if (DateTime.now().difference(stamp.at).inSeconds > 15) {
      _echo.remove(uid);
      return false;
    }
    return stamp.fingerprint == _fingerprint(tags);
  }

  void _rememberEcho(String uid, List<String> tags) {
    _echo[uid] = _EchoStamp(_fingerprint(tags), _revision[uid] ?? -1, DateTime.now());
  }
  /// 乐观更新；PATCH 失败回滚旧值返回 false；离线/网络错进重试队列
  Future<bool> setTags(String uid, List<String> tags) async {
    final clean = _normalize(tags);
    final rawOld = tagsByUid[uid];
    final old = rawOld == null ? null : List<String>.from(rawOld);
    tagsByUid[uid] = clean;
    _rebuildAllTags();
    final token = _token();
    if (token.isEmpty) {
      debugPrint('[TagSync] setTags 无登录令牌 uid=$uid tags=$clean，进重试队列');
      _rollback(uid, old);
      _enqueueRetry(uid, clean);
      return false;
    }
    try {
      await _api.patchDeviceTags(token, uid, clean);
    } on OmApiException catch (e) {
      debugPrint('[TagSync] setTags 服务端拒绝 uid=$uid tags=$clean '
          'status=${e.statusCode} msg=${e.message} body=${e.bodySnippet}');
      _rollback(uid, old);
      if (!e.isMissingEndpoint) return false;
      debugPrint('[TagSync] 母服缺 tags 端点(404)，不进重试队列 uid=$uid');
      return false;
    } catch (e) {
      debugPrint('[TagSync] setTags 网络异常 uid=$uid tags=$clean '
          '${e.runtimeType} $e，回滚并进重试队列');
      _rollback(uid, old);
      _enqueueRetry(uid, clean);
      return false;
    }
    _rememberEcho(uid, clean);
    return true;
  }
  String _token() {
    try {
      return OmAuth.instance.accessToken ?? '';
    } catch (e) {
      debugPrint('[TagSync] 读取登录令牌失败: ${e.runtimeType} $e');
      return '';
    }
  }

  void _rollback(String uid, List<String>? old) {
    if (old == null) {
      tagsByUid.remove(uid);
    } else {
      tagsByUid[uid] = old;
    }
    _rebuildAllTags();
  }

  void _enqueueRetry(String uid, List<String> tags) {
    _retryQueue[uid] = _TagWrite(uid, List<String>.from(tags));
    _ensureRetryTimer();
  }

  void _ensureRetryTimer() {
    if (_retryTimer != null) return;
    _retryTimer =
        Timer.periodic(const Duration(seconds: 5), (_) => unawaited(flushRetry()));
  }
  /// 定时/重连时冲刷重试队列；成功才落内存态，失败保队列等下次
  Future<void> flushRetry() async {
    if (_retryQueue.isEmpty) return;
    final token = _token();
    if (token.isEmpty) return;
    for (final w in _retryQueue.values.toList()) {
      try {
        await _api.patchDeviceTags(token, w.uid, w.tags);
        _retryQueue.remove(w.uid);
        tagsByUid[w.uid] = List<String>.from(w.tags);
        _rebuildAllTags();
        _rememberEcho(w.uid, w.tags);
      } on OmApiException catch (e) {
        w.attempts += 1;
        debugPrint('[TagSync] 重试被拒 uid=${w.uid} status=${e.statusCode} '
            'attempts=${w.attempts} msg=${e.message}');
        // 服务端明确拒绝或端点缺失，重试无意义，丢弃并留日志
        _retryQueue.remove(w.uid);
      } catch (e) {
        w.attempts += 1;
        debugPrint('[TagSync] 重试网络异常 uid=${w.uid} attempts=${w.attempts} '
            '${e.runtimeType} $e');
      }
    }
    if (_retryQueue.isEmpty) {
      _retryTimer?.cancel();
      _retryTimer = null;
    }
  }
  /// 批量打标：逐台整组替换，任一台失败即返回 false
  Future<bool> addTagToDevices(List<String> uids, String tag) async {
    final t = tag.trim();
    if (t.isEmpty || uids.isEmpty) return false;
    var ok = true;
    for (final uid in uids) {
      final cur = List<String>.from(tagsByUid[uid] ?? const <String>[]);
      if (cur.contains(t)) continue;
      cur.add(t);
      if (!await setTags(uid, cur)) ok = false;
    }
    return ok;
  }

  /// 批量摘标：逐台整组替换，任一台失败即返回 false
  Future<bool> removeTagFromDevices(List<String> uids, String tag) async {
    final t = tag.trim();
    if (t.isEmpty || uids.isEmpty) return false;
    var ok = true;
    for (final uid in uids) {
      final cur = List<String>.from(tagsByUid[uid] ?? const <String>[]);
      if (!cur.contains(t)) continue;
      cur.remove(t);
      if (!await setTags(uid, cur)) ok = false;
    }
    return ok;
  }
  /// 订阅 om 事件流；断线由 OmApi 内部指数退避重连
  void startSse(String bearerToken) {
    stopSse();
    if (bearerToken.isEmpty) {
      debugPrint('[TagSync] startSse 令牌为空，跳过订阅');
      return;
    }
    _sseToken = bearerToken;
    _sseSub = _api.subscribeEvents(bearerToken).listen(
      _onSseEvent,
      onError: (Object e) {
        debugPrint('[TagSync] SSE 流错误: ${e.runtimeType} $e');
      },
      onDone: () => debugPrint('[TagSync] SSE 流结束'),
      cancelOnError: false,
    );
    unawaited(flushRetry());
  }
  void _onSseEvent(OmSseEvent ev) {
    try {
      final d = ev.data;
      if (ev.event == 'ping' || ev.event == 'seats') return;
      if (d.containsKey('uid')) {
        final uid = (d['uid'] ?? '').toString();
        final rawOnline = d['online'];
        final status = (d['status'] ?? '').toString().toLowerCase();
        final bool? online = rawOnline is bool
            ? rawOnline
            : (status.isEmpty ? null : status == 'active');
        if (uid.isNotEmpty && online != null) {
          onDeviceState?.call(uid, online);
        }
      }
      // 母服暂无 tag 变更事件；一旦带 uid+tags 就按去重回声吸收
      _absorbTagsFromMap(d, ev.event);
    } catch (e) {
      debugPrint('[TagSync] SSE 事件处理异常: ${e.runtimeType} $e '
          'event=${ev.event} data=${jsonEncode(ev.data)}');
    }
  }

  void _absorbTagsFromMap(Map<String, dynamic> d, String source) {
    final uid = (d['uid'] ?? '').toString();
    final rawTags = d['tags'];
    if (uid.isEmpty || rawTags is! List) return;
    _applyRemote(
      uid,
      rawTags.map((e) => e.toString()).toList(),
      _parseRevision(d['revision']),
      source,
    );
  }
  /// 停 SSE 订阅（不清内存态，登出请用 reset）
  void stopSse() {
    final sub = _sseSub;
    _sseSub = null;
    if (sub == null) return;
    debugPrint('[TagSync] stopSse 断开订阅 tokenLen=${_sseToken.length}');
    _sseToken = '';
    try {
      unawaited(sub.cancel());
    } catch (e) {
      debugPrint('[TagSync] stopSse 取消订阅异常: ${e.runtimeType} $e');
    }
  }

  /// 登出：清空内存态 + 停 SSE + 丢弃重试队列与回声缓存
  void reset() {
    stopSse();
    _retryTimer?.cancel();
    _retryTimer = null;
    _retryQueue.clear();
    _echo.clear();
    _revision.clear();
    tagsByUid.clear();
    allTags.clear();
    onDeviceState = null;
  }
}
