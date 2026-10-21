import 'dart:convert';

import 'package:flutter_hbb/models/om_auth.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// 登出: 清内存 token + 落盘清空 + loginState=false。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  Map<String, String> store = <String, String>{};

  OmAuth makeAuth() => OmAuth.forTest(
        httpClient: MockClient((req) async => http.Response(
            jsonEncode({
              'data': {'access_token': 'tok-out', 'expires_in': 3600}
            }),
            200)),
        readOption: (key) => store[key] ?? '',
        writeOption: (key, value) async {
          store[key] = value;
        },
      );

  setUp(() {
    store = <String, String>{};
  });

  test('signOut: 内存 token 清空 + 落盘清空 + loginState=false', () async {
    final auth = makeAuth();
    expect(await auth.login('a@b.com', 'pw'), isTrue);
    expect(store['om-auth-jwt'], 'tok-out');

    await auth.signOut();
    expect(auth.accessToken, isNull);
    expect(auth.email, isEmpty);
    expect(auth.isLoggedIn, isFalse);
    expect(store['om-auth-jwt'], isEmpty);
    expect(store['om-auth-email'], isEmpty);
    expect(store['om-auth-exp'], isEmpty);
  });

  test('signOut 后 restore 不再恢复登录', () async {
    final auth = makeAuth();
    await auth.login('a@b.com', 'pw');
    await auth.signOut();

    final auth2 = makeAuth();
    expect(await auth2.restore(), isFalse);
    expect(auth2.isLoggedIn, isFalse);
    expect(auth2.accessToken, isNull);
  });

  test('handleUnauthorized: 401 路径清登录态回登录页', () async {
    final auth = makeAuth();
    await auth.login('a@b.com', 'pw');
    expect(auth.isLoggedIn, isTrue);

    await auth.handleUnauthorized();
    expect(auth.isLoggedIn, isFalse);
    expect(auth.accessToken, isNull);
    expect(store['om-auth-jwt'], isEmpty);
  });

  test('signOut 不清设备标识 (稳定复用)', () async {
    final auth = makeAuth();
    await auth.login('a@b.com', 'pw');
    final device = store['om-auth-device'];
    expect(device, isNotEmpty);

    await auth.signOut();
    expect(store['om-auth-device'], device);
  });
}
