import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart' show debugPrint;
import 'package:get/get.dart';
import 'package:http/http.dart' as http;

import 'platform_model.dart';

/// 母服登录态 (与 SessionHost/OmApiClient.cs 同口径)。
/// token 运行期只驻内存 (accessToken), 落盘走 rustdesk 本地配置;
/// 禁止把 token 写进日志 / 窗口标题 / 启动参数。
class OmAuth {
  OmAuth._();

  static final OmAuth instance = OmAuth._();

  /// 登录端点, 与 NativeAdmissionChain.VipBase / LiveDurtHost.LoginBase 同源。
  static const String _loginUrl = 'https://auth.darra.xyz/portal/login';

  static const String _keyToken = 'om-auth-jwt';
  static const String _keyEmail = 'om-auth-email';
  static const String _keyExpire = 'om-auth-exp';
  static const String _keyDevice = 'om-auth-device';

  /// 登录态开关, 供 Obx 订阅即时切换登录页 / 主界面。
  final RxBool loginState = false.obs;

  String? _accessToken;
  String _email = '';

  /// 内存 token, 连接与设备 API 取用。
  String? get accessToken => _accessToken;

  bool get isLoggedIn => loginState.value;

  String get email => _email;

  /// 邮箱密码登录; true=成功。
  Future<bool> login(String email, String password) async {
    final mail = email.trim();
    if (mail.isEmpty || password.isEmpty) return false;
    try {
      final deviceId = await _deviceId();
      final resp = await http
          .post(
            Uri.parse(_loginUrl),
            headers: {'Content-Type': 'application/json'},
            body: jsonEncode({
              'email': mail,
              'password': password,
              'device_id': deviceId,
            }),
          )
          .timeout(const Duration(seconds: 20));
      final token = _parseToken(resp.body);
      if (token == null) {
        debugPrint('[OmAuth] 登录失败: 响应缺 token');
        return false;
      }
      _accessToken = token;
      _email = mail;
      await _persist(token, mail, _parseExpiry(resp.body));
      loginState.value = true;
      return true;
    } catch (ex) {
      debugPrint('[OmAuth] 登录异常: ${_safeError(ex)}');
      return false;
    }
  }

  /// 清内存 + 存储, 回登录页。
  Future<void> signOut() async {
    _accessToken = null;
    _email = '';
    loginState.value = false;
    try {
      await bind.mainSetLocalOption(key: _keyToken, value: '');
      await bind.mainSetLocalOption(key: _keyEmail, value: '');
      await bind.mainSetLocalOption(key: _keyExpire, value: '');
    } catch (ex) {
      debugPrint('[OmAuth] 清除登录态失败: ${_safeError(ex)}');
    }
  }

  /// 启动时恢复记住的登录; 不抛异常, 恢复失败一律按未登录。
  Future<bool> restore() async {
    try {
      final token = bind.mainGetLocalOption(key: _keyToken);
      if (token.isEmpty) {
        loginState.value = false;
        return false;
      }
      final expRaw = bind.mainGetLocalOption(key: _keyExpire);
      final expMs = expRaw.isEmpty ? null : int.tryParse(expRaw);
      if (expRaw.isNotEmpty && expMs == null) {
        await signOut();
        return false;
      }
      if (expMs != null &&
          DateTime.now().millisecondsSinceEpoch >= expMs) {
        await signOut();
        return false;
      }
      _accessToken = token;
      _email = bind.mainGetLocalOption(key: _keyEmail);
      loginState.value = true;
      return true;
    } catch (ex) {
      debugPrint('[OmAuth] 恢复登录态失败: ${_safeError(ex)}');
      _accessToken = null;
      _email = '';
      loginState.value = false;
      return false;
    }
  }

  /// 设备标识: 无则生成一次 32 位 hex, 之后稳定复用。
  Future<String> _deviceId() async {
    try {
      final saved = bind.mainGetLocalOption(key: _keyDevice);
      if (saved.isNotEmpty) return saved;
      final id = _randomHex32();
      await bind.mainSetLocalOption(key: _keyDevice, value: id);
      return id;
    } catch (ex) {
      debugPrint('[OmAuth] 读取设备标识失败: ${_safeError(ex)}');
      return _randomHex32();
    }
  }

  /// 16 字节随机数 -> 32 位 hex 设备标识。
  String _randomHex32() {
    final rnd = Random.secure();
    return List<int>.generate(16, (_) => rnd.nextInt(256))
        .map((b) => b.toRadixString(16).padLeft(2, '0'))
        .join();
  }

  Future<void> _persist(String token, String mail, DateTime? exp) async {
    await bind.mainSetLocalOption(key: _keyToken, value: token);
    await bind.mainSetLocalOption(key: _keyEmail, value: mail);
    await bind.mainSetLocalOption(
        key: _keyExpire,
        value: exp == null ? '' : '${exp.millisecondsSinceEpoch}');
  }

  /// 与 OmApiClient 同口径: data.access_token -> data.token -> 顶层 token。
  String? _parseToken(String text) {
    final root = _tryDecode(text);
    if (root == null) return null;
    String? token;
    final data = root['data'];
    if (data is Map) {
      token = _nonEmpty(data['access_token']) ?? _nonEmpty(data['token']);
    }
    return token ?? _nonEmpty(root['token']);
  }

  /// data.expires_in(秒) -> 过期时刻。
  DateTime? _parseExpiry(String text) {
    final root = _tryDecode(text);
    if (root == null) return null;
    final data = root['data'];
    if (data is Map) {
      final exp = data['expires_in'];
      if (exp is num && exp > 0) {
        return DateTime.now()
            .toUtc()
            .add(Duration(seconds: exp.toInt()));
      }
    }
    return null;
  }

  String? _nonEmpty(Object? v) {
    if (v is! String) return null;
    final t = v.trim();
    return t.isEmpty ? null : t;
  }

  Map<String, dynamic>? _tryDecode(String text) {
    final t = text.trim();
    if (t.isEmpty || !t.startsWith('{')) return null;
    try {
      final v = jsonDecode(t);
      return v is Map<String, dynamic> ? v : null;
    } catch (ex) {
      // 不记响应正文: 登录响应可能含 token。
      debugPrint('[OmAuth] 响应解析失败: ${ex.runtimeType}');
      return null;
    }
  }

  /// 只暴露不含正文的错误信息, 防止 token / 密码进日志。
  String _safeError(Object ex) {
    if (ex is TimeoutException) return ex.message ?? '超时';
    if (ex is http.ClientException) return ex.message;
    return ex.runtimeType.toString();
  }
}
