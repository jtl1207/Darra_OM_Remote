import 'dart:async';
import 'dart:convert';

import 'package:flutter_hbb/models/om_auth.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// 内存本地配置: 模拟 bind.mainSetLocalOption/mainGetLocalOption。
Map<String, String> _store() => <String, String>{};

OmOptionReader _reader(Map<String, String> store) =>
    (key) => store[key] ?? '';

OmOptionWriter _writer(Map<String, String> store) => (key, value) async {
      store[key] = value;
    };

/// 固定假时钟, 从 [start] 起。
DateTime Function() _clock(DateTime start) {
  var t = start;
  return () => t;
}

http.Client _okClient(String body) =>
    MockClient((req) async => http.Response(body, 200));

http.Client _echoClient(String body, void Function(http.Request) onReq) =>
    MockClient((req) async {
      onReq(req);
      return http.Response(body, 200);
    });

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('登录成功: 写内存 token 并落盘', () async {
    final store = _store();
    var seenUrl = '';
    final auth = OmAuth.forTest(
      httpClient: _echoClient(
        jsonEncode({
          'data': {'access_token': 'tok-ok', 'expires_in': 3600}
        }),
        (req) => seenUrl = req.url.toString(),
      ),
      readOption: _reader(store),
      writeOption: _writer(store),
      clock: _clock(DateTime.fromMillisecondsSinceEpoch(1700000000000, isUtc: true)),
    );
    final ok = await auth.login('a@b.com', 'pw');
    expect(ok, isTrue);
    expect(seenUrl, 'https://auth.darra.xyz/portal/login');
    expect(auth.isLoggedIn, isTrue);
    expect(auth.accessToken, 'tok-ok');
    expect(auth.email, 'a@b.com');
    expect(store['om-auth-jwt'], 'tok-ok');
    expect(store['om-auth-email'], 'a@b.com');
    expect(store['om-auth-exp'], isNotEmpty);
  });

  test('登录失败: 响应缺 token 则不置登录态', () async {
    final store = _store();
    final auth = OmAuth.forTest(
      httpClient: _okClient(jsonEncode({'error': 'denied'})),
      readOption: _reader(store),
      writeOption: _writer(store),
    );
    final ok = await auth.login('a@b.com', 'pw');
    expect(ok, isFalse);
    expect(auth.isLoggedIn, isFalse);
    expect(auth.accessToken, isNull);
    expect(store['om-auth-jwt'] ?? '', isEmpty);
  });

  test('空参: 邮箱或密码为空直接失败且不发请求', () async {
    final store = _store();
    var called = false;
    final auth = OmAuth.forTest(
      httpClient: MockClient((req) async {
        called = true;
        return http.Response('{}', 200);
      }),
      readOption: _reader(store),
      writeOption: _writer(store),
    );
    expect(await auth.login('', 'pw'), isFalse);
    expect(await auth.login('a@b.com', ''), isFalse);
    expect(await auth.login('   ', '  '), isFalse);
    expect(called, isFalse);
    expect(auth.isLoggedIn, isFalse);
  });

  test('token 解析: data.access_token / data.token / 顶层 token', () async {
    Future<String?> via(String body) async {
      final store = _store();
      final auth = OmAuth.forTest(
        httpClient: _okClient(body),
        readOption: _reader(store),
        writeOption: _writer(store),
      );
      final ok = await auth.login('a@b.com', 'pw');
      return ok ? auth.accessToken : null;
    }

    expect(await via(jsonEncode({
      'data': {'access_token': 'A'}
    })), 'A');
    expect(await via(jsonEncode({
      'data': {'token': 'B'}
    })), 'B');
    expect(await via(jsonEncode({'token': 'C'})), 'C');
    expect(await via(jsonEncode({
      'data': {'access_token': '', 'token': 'D'}
    })), 'D');
    expect(await via('not-json'), isNull);
  });

  test('过期 restore 失效: 自动 signOut 并清落盘', () async {
    final store = <String, String>{
      'om-auth-jwt': 'old-token',
      'om-auth-email': 'a@b.com',
      'om-auth-exp': '1000',
    };
    final auth = OmAuth.forTest(
      readOption: _reader(store),
      writeOption: _writer(store),
      // 假时间 2000 > exp 1000 => 过期
      clock: () => DateTime.fromMillisecondsSinceEpoch(2000, isUtc: true),
    );
    final ok = await auth.restore();
    expect(ok, isFalse);
    expect(auth.isLoggedIn, isFalse);
    expect(auth.accessToken, isNull);
    expect(store['om-auth-jwt'], isEmpty);
    expect(store['om-auth-email'], isEmpty);
    expect(store['om-auth-exp'], isEmpty);
  });

  test('未过期 restore 成功: 读回 token 与邮箱', () async {
    final store = <String, String>{
      'om-auth-jwt': 'live-token',
      'om-auth-email': 'a@b.com',
      'om-auth-exp': '9999999999999',
    };
    final auth = OmAuth.forTest(
      readOption: _reader(store),
      writeOption: _writer(store),
      clock: () => DateTime.fromMillisecondsSinceEpoch(1000, isUtc: true),
    );
    final ok = await auth.restore();
    expect(ok, isTrue);
    expect(auth.isLoggedIn, isTrue);
    expect(auth.accessToken, 'live-token');
    expect(auth.email, 'a@b.com');
  });

  test('signOut 清 token: 内存与落盘同时清空', () async {
    final store = _store();
    final auth = OmAuth.forTest(
      httpClient: _okClient(jsonEncode({
        'data': {'access_token': 'tok-x', 'expires_in': 3600}
      })),
      readOption: _reader(store),
      writeOption: _writer(store),
    );
    await auth.login('a@b.com', 'pw');
    expect(auth.isLoggedIn, isTrue);

    await auth.signOut();
    expect(auth.isLoggedIn, isFalse);
    expect(auth.accessToken, isNull);
    expect(auth.email, isEmpty);
    expect(store['om-auth-jwt'], isEmpty);
    expect(store['om-auth-email'], isEmpty);
    expect(store['om-auth-exp'], isEmpty);
  });

  test('超时: 注入短 Duration, 不真网真等即失败', () async {
    final store = _store();
    final auth = OmAuth.forTest(
      httpClient: MockClient((req) async {
        // 永不完成的请求, 由注入超时截断。
        return await Completer<http.Response>().future;
      }),
      loginTimeout: const Duration(milliseconds: 1),
      readOption: _reader(store),
      writeOption: _writer(store),
    );
    final ok = await auth.login('a@b.com', 'pw');
    expect(ok, isFalse);
    expect(auth.isLoggedIn, isFalse);
  });
}
