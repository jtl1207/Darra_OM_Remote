// om-master API 层：设备清单 / tag 整组替换 / SSE 事件订阅
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:http/http.dart' as http;

import 'om_auth.dart';

/// 单次 API 调用失败（带 HTTP 状态码与响应片段，供上层区分 404 缺口）
class OmApiException implements Exception {
  final int statusCode;
  final String message;
  final String? bodySnippet;
  OmApiException(this.statusCode, this.message, this.bodySnippet);
  bool get isMissingEndpoint => statusCode == 404;
  @override
  String toString() =>
      'OmApiException(status=$statusCode, message=$message, body=$bodySnippet)';
}
/// 设备清单单条（om-master toOmDevice 响应）
class OmDeviceData {
  final String uid;
  final String name;
  final bool online;
  final List<String> tags;
  final Map<String, dynamic> raw;
  OmDeviceData({
    required this.uid,
    required this.name,
    required this.online,
    required this.tags,
    required this.raw,
  });
  /// 解析 om-master 设备条目；tags/revision 当前母服可缺，缺则空列表
  static OmDeviceData fromJson(Map<String, dynamic> d) {
    final uid = (d['uid'] ?? '').toString();
    final name = (d['custom_name'] ?? d['name'] ?? uid).toString();
    final status = (d['status'] ?? '').toString().toLowerCase();
    final onlineRaw = d['online'];
    final bool online = onlineRaw is bool ? onlineRaw : status == 'active';
    return OmDeviceData(
      uid: uid,
      name: name,
      online: online,
      tags: _parseTags(d['tags']),
      raw: d,
    );
  }

  static List<String> _parseTags(dynamic raw) {
    final out = <String>[];
    if (raw is List) {
      for (final t in raw) {
        final s = t?.toString().trim() ?? '';
        if (s.isNotEmpty && !out.contains(s)) out.add(s);
      }
    }
    return out;
  }
}

/// SSE 事件：event 名 + 解析后的 data 对象
class OmSseEvent {
  final String event;
  final Map<String, dynamic> data;
  OmSseEvent(this.event, this.data);
  @override
  String toString() => 'OmSseEvent($event, $data)';
}

/// SSE 事件流注入点: 测试可换假事件流, 生产走 HttpClient 长连接。
typedef OmSseFactory = Stream<OmSseEvent> Function(String bearerToken);

/// 401 处理注入点: 默认清登录态回登录页。
typedef OmUnauthorizedHandler = Future<void> Function(String who);

/// om-master REST / SSE 访问入口（无状态，全部方法显式传 token）
class OmApi {
  OmApi({
    http.Client? httpClient,
    OmSseFactory? sseFactory,
    OmUnauthorizedHandler? onUnauthorized,
  })  : _client = httpClient ?? http.Client(),
        _sseFactory = sseFactory,
        _onUnauthorized = onUnauthorized ?? _defaultOnUnauthorized;

  static const String omBase = 'https://om.darra.xyz/api';

  /// HTTP 客户端可注入 (测试用 MockClient)。
  final http.Client _client;

  /// SSE 事件流可注入 (测试用假事件流)。
  final OmSseFactory? _sseFactory;

  /// 401 处理可注入; 默认走 OmAuth.handleUnauthorized 回登录页。
  final OmUnauthorizedHandler _onUnauthorized;

  static Future<void> _defaultOnUnauthorized(String who) {
    debugPrint('[OmApi] $who token 失效, 回登录页');
    return OmAuth.instance.handleUnauthorized();
  }

  Map<String, String> _authHeaders(String bearerToken,
      {String? contentType}) {
    final h = <String, String>{'Accept': 'application/json'};
    if (bearerToken.isNotEmpty) h['Authorization'] = 'Bearer $bearerToken';
    if (contentType != null) h['Content-Type'] = contentType;
    return h;
  }

  String _snip(String body) =>
      body.length > 200 ? body.substring(0, 200) : body;

  /// 响应体按 UTF-8 解码 (JSON 规范编码), 不依赖 content-type 是否带 charset。
  String _bodyText(http.Response res) =>
      utf8.decode(res.bodyBytes, allowMalformed: true);

  /// token 失效 (401): 清登录态回登录页
  void _signOutOnUnauthorized(String who) {
    _onUnauthorized(who).catchError(
        (e) => debugPrint('[OmApi] $who 401 处理异常: ${e.runtimeType} $e'));
  }

  /// GET /om/devices —— 返回设备清单（tags/revision 母服当前可缺）
  Future<List<OmDeviceData>> listDevices(String bearerToken) async {
    final uri = Uri.parse('$omBase/om/devices');
    http.Response res;
    try {
      res = await _client.get(uri, headers: _authHeaders(bearerToken));
    } catch (e) {
      debugPrint('[OmApi] listDevices 网络异常: ${e.runtimeType} $e uri=$uri');
      rethrow;
    }
    if (res.statusCode < 200 || res.statusCode >= 300) {
      debugPrint('[OmApi] listDevices HTTP ${res.statusCode}: ${_snip(_bodyText(res))}');
      if (res.statusCode == 401) _signOutOnUnauthorized('listDevices');
      throw OmApiException(res.statusCode, 'listDevices 失败', _snip(_bodyText(res)));
    }
    return _parseDeviceList(_bodyText(res), uri);
  }
  /// 兼容 `data` / `devices` / 顶层数组三种包裹
  List<OmDeviceData> _parseDeviceList(String body, Uri uri) {
    dynamic decoded;
    try {
      decoded = jsonDecode(body.isEmpty ? '[]' : body);
    } catch (e) {
      debugPrint('[OmApi] listDevices JSON 解析失败: ${e.runtimeType} $e uri=$uri');
      throw OmApiException(-1, 'listDevices 响应非 JSON', _snip(body));
    }
    List<dynamic>? arr;
    if (decoded is List) {
      arr = decoded;
    } else if (decoded is Map) {
      final d = decoded['data'];
      final dv = decoded['devices'];
      if (d is List) {
        arr = d;
      } else if (dv is List) {
        arr = dv;
      }
    }
    if (arr == null) {
      debugPrint('[OmApi] listDevices 缺设备数组: ${_snip(body)}');
      return const [];
    }
    final out = <OmDeviceData>[];
    for (final item in arr) {
      if (item is Map<String, dynamic>) {
        out.add(OmDeviceData.fromJson(item));
      } else if (item is Map) {
        out.add(OmDeviceData.fromJson(Map<String, dynamic>.from(item)));
      } else {
        debugPrint('[OmApi] 跳过非法设备条目: ${item.runtimeType} $item');
      }
    }
    return out;
  }
  /// PATCH /om/devices/<uid>/tags —— 整组替换语义，body 仅 `{"tags":[...]}`
  Future<void> patchDeviceTags(
      String bearerToken, String uid, List<String> tags) async {
    final clean = <String>[];
    for (final t in tags) {
      final s = t.trim();
      if (s.isNotEmpty && !clean.contains(s)) clean.add(s);
    }
    final uri = Uri.parse('$omBase/om/devices/${Uri.encodeComponent(uid)}/tags');
    http.Response res;
    try {
      res = await _client.patch(uri,
          headers: _authHeaders(bearerToken, contentType: 'application/json'),
          body: jsonEncode({'tags': clean}));
    } catch (e) {
      debugPrint('[OmApi] patchDeviceTags 网络异常: ${e.runtimeType} $e uid=$uid');
      rethrow;
    }
    if (res.statusCode < 200 || res.statusCode >= 300) {
      debugPrint('[OmApi] patchDeviceTags HTTP ${res.statusCode} uid=$uid '
          'tags=$clean body=${_snip(_bodyText(res))}');
      if (res.statusCode == 401) _signOutOnUnauthorized('patchDeviceTags');
      throw OmApiException(
          res.statusCode, 'patchDeviceTags 失败', _snip(_bodyText(res)));
    }
  }

  /// GET /om/events —— SSE 长流，断线自动重连（指数退避封顶 60s）
  /// SSE 连接可注入: 测试传 [sseFactory] 假事件流, 生产走 HttpClient 长连接。
  Stream<OmSseEvent> subscribeEvents(String bearerToken) {
    final injected = _sseFactory;
    if (injected != null) return injected(bearerToken);
    final controller = StreamController<OmSseEvent>();
    var stopped = false;
    var attempt = 0;

    Future<void> dial() async {
      if (stopped) return;
      HttpClient? client;
      try {
        client = HttpClient()..connectionTimeout = const Duration(seconds: 15);
        final uri = Uri.parse('$omBase/om/events'
            '?token=${Uri.encodeQueryComponent(bearerToken)}');
        final req = await client.getUrl(uri);
        req.headers.set(HttpHeaders.authorizationHeader, 'Bearer $bearerToken');
        req.headers.set(HttpHeaders.acceptHeader, 'text/event-stream');
        req.headers.set(HttpHeaders.cacheControlHeader, 'no-cache');
        final res = await req.close();
        if (res.statusCode < 200 || res.statusCode >= 300) {
          throw OmApiException(res.statusCode, 'subscribeEvents 非 2xx', null);
        }
        attempt = 0;
        final parser = _SseParser();
        await for (final line
            in res.transform(utf8.decoder).transform(const LineSplitter())) {
          if (stopped) break;
          parser.feed(line, controller.add);
        }
      } catch (e) {
        debugPrint('[OmApi] SSE 连接异常: ${e.runtimeType} $e attempt=$attempt '
            'tokenLen=${bearerToken.length}');
      } finally {
        try {
          client?.close(force: true);
        } catch (e) {
          debugPrint('[OmApi] SSE 关闭异常: ${e.runtimeType} $e');
        }
      }
      if (stopped) return;
      attempt += 1;
      final secs = attempt >= 6 ? 60 : (1 << (attempt - 1));
      debugPrint('[OmApi] SSE ${secs}s 后重连 attempt=$attempt');
      Timer(Duration(seconds: secs), dial);
    }

    controller.onCancel = () {
      stopped = true;
    };
    unawaited(dial());
    return controller.stream;
  }
}

/// SSE 帧行解析器：按 event/data 累积，空行派发一条
class _SseParser {
  String _event = '';
  final StringBuffer _data = StringBuffer();
  void feed(String line, void Function(OmSseEvent) emit) {
    if (line.isEmpty) {
      final data = _data.toString().trim();
      final eventName = _event.isEmpty ? 'message' : _event;
      _event = '';
      _data.clear();
      if (data.isEmpty) return;
      dynamic decoded;
      try {
        decoded = jsonDecode(data);
      } catch (e) {
        debugPrint('[OmApi] SSE data 非 JSON 跳过: ${e.runtimeType} $e raw=$data');
        return;
      }
      final map = decoded is Map
          ? Map<String, dynamic>.from(decoded)
          : <String, dynamic>{'value': decoded};
      emit(OmSseEvent(eventName, map));
      return;
    }
    if (line.startsWith(':')) return;
    final i = line.indexOf(':');
    final field = i < 0 ? line : line.substring(0, i);
    var value = i < 0 ? '' : line.substring(i + 1);
    if (value.startsWith(' ')) value = value.substring(1);
    if (field == 'event') {
      _event = value;
    } else if (field == 'data') {
      if (_data.isNotEmpty) _data.writeln();
      _data.write(value);
    }
  }
}
