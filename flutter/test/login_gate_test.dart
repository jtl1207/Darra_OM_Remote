import 'package:flutter/material.dart';
import 'package:flutter_hbb/common/widgets/login_page.dart';
import 'package:flutter_hbb/models/om_auth.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:get/get.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// 登录门状态机: loginState 切换驱动 登录页 / 主界面。
/// 复刻 lib/main.dart 的 home Obx 写法, 注入测试实例避免碰 FFI。
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  OmAuth makeAuth(Map<String, String> store) => OmAuth.forTest(
        // 恒失败的假 HTTP: 禁真网真等。
        httpClient: MockClient((req) async => http.Response('{"error":"x"}', 200)),
        readOption: (key) => store[key] ?? '',
        writeOption: (key, value) async {
          store[key] = value;
        },
      );

  test('loginState 驱动 isLoggedIn 状态机', () {
    final auth = makeAuth(<String, String>{});
    expect(auth.isLoggedIn, isFalse);
    auth.loginState.value = true;
    expect(auth.isLoggedIn, isTrue);
    auth.loginState.value = false;
    expect(auth.isLoggedIn, isFalse);
  });

  testWidgets('未登录显示登录页, 登录后显示主界面', (tester) async {
    final store = <String, String>{};
    final auth = makeAuth(store);

    Widget gate() => GetMaterialApp(
          home: Obx(() {
            if (!auth.isLoggedIn) return LoginPage(auth: auth);
            return const Text('MAIN');
          }),
        );

    await tester.pumpWidget(gate());
    expect(find.byType(LoginPage), findsOneWidget);
    expect(find.text('MAIN'), findsNothing);

    // 切登录态 -> 主界面
    auth.loginState.value = true;
    await tester.pump();
    expect(find.byType(LoginPage), findsNothing);
    expect(find.text('MAIN'), findsOneWidget);

    // 切回 -> 登录页
    auth.loginState.value = false;
    await tester.pump();
    expect(find.byType(LoginPage), findsOneWidget);
    expect(find.text('MAIN'), findsNothing);
  });

  testWidgets('登录页输错密码仍停在登录页', (tester) async {
    final store = <String, String>{};
    final auth = makeAuth(store);
    await tester.pumpWidget(GetMaterialApp(
      home: Obx(() {
        if (!auth.isLoggedIn) return LoginPage(auth: auth);
        return const Text('MAIN');
      }),
    ));

    await tester.enterText(find.byType(TextField).first, 'a@b.com');
    await tester.enterText(find.byType(TextField).last, 'bad');
    // 假 HTTP 返回缺 token -> login false, 仍留登录页。
    await tester.tap(find.byType(ElevatedButton));
    await tester.pump(const Duration(milliseconds: 50));
    expect(auth.isLoggedIn, isFalse);
    expect(find.byType(LoginPage), findsOneWidget);
    expect(find.text('MAIN'), findsNothing);
  });
}
