// 设置收口范围断言: 仅留 语言/主题/显示/关于/退出登录, 无 Keyboard shortcuts 与安全/网络/账户/打印 tab。
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

String _readSource(String relPath) {
  final f = File(relPath);
  if (!f.existsSync()) {
    // 测试从包根运行; 兜底用当前目录递归定位
    final alt = Directory.current.listSync(recursive: false).whereType<File>();
    for (final e in alt) {
      if (e.path.endsWith(relPath.replaceAll('/', Platform.pathSeparator))) {
        return e.readAsStringSync();
      }
    }
    fail('找不到源文件: $relPath');
  }
  return f.readAsStringSync();
}

void main() {
  final desktop = _readSource('lib/desktop/pages/desktop_setting_page.dart');
  final mobile = _readSource('lib/mobile/pages/settings_page.dart');

  group('desktop 设置收口', () {
    test('tabKeys 仅 general/display/about', () {
      final m = RegExp(r'tabKeys\s*=\s*\[(.*?)\];', dotAll: true)
          .firstMatch(desktop);
      expect(m, isNotNull, reason: 'tabKeys 列表应存在');
      final body = m!.group(1)!;
      expect(body.contains('SettingsTabKey.general'), isTrue);
      expect(body.contains('SettingsTabKey.display'), isTrue);
      expect(body.contains('SettingsTabKey.about'), isTrue);
      expect(body.contains('SettingsTabKey.safety'), isFalse);
      expect(body.contains('SettingsTabKey.network'), isFalse);
      expect(body.contains('SettingsTabKey.account'), isFalse);
      expect(body.contains('SettingsTabKey.printer'), isFalse);
    });

    test('General 仅 Language/Theme/Logout', () {
      expect(desktop.contains("_Card(title: 'Language'"), isTrue);
      expect(desktop.contains("_Card(title: 'Theme'"), isTrue);
      expect(desktop.contains("_Card(title: 'Logout'"), isTrue);
      expect(desktop.contains('OmAuth.instance.signOut()'), isTrue);
    });

    test('无 Keyboard shortcuts 卡与已删 tab 枚举值', () {
      expect(desktop.contains('Keyboard shortcuts'), isFalse);
      expect(desktop.contains('keyboardShortcuts'), isFalse);
      expect(desktop.contains('SettingsTabKey.safety'), isFalse);
      expect(desktop.contains('SettingsTabKey.network'), isFalse);
      expect(desktop.contains('SettingsTabKey.account'), isFalse);
      expect(desktop.contains('SettingsTabKey.printer'), isFalse);
    });
  });

  group('mobile 设置收口', () {
    test('仅保留 Language/Theme/Display/About/退出登录', () {
      expect(mobile.contains("translate('Language')"), isTrue);
      expect(mobile.contains("'Light Theme'"), isTrue);
      expect(mobile.contains('Display Settings'), isTrue);
      expect(mobile.contains('translate("About")'), isTrue);
      expect(mobile.contains("translate('Logout')"), isTrue);
    });

    test('无 Keyboard shortcuts 与多余设置项', () {
      expect(mobile.contains('Keyboard shortcuts'), isFalse);
      expect(mobile.contains('MobileKeyboardShortcutsPage'), isFalse);
      expect(mobile.contains('ID/Relay Server'), isFalse);
      expect(mobile.contains('Socks5'), isFalse);
      expect(mobile.contains('2FA'), isFalse);
    });
  });
}
