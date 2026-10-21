import 'dart:js' as js;
// cycle imports, maybe we can improve this
import 'package:flutter_hbb/consts.dart';

final isAndroid_ = false;
final isIOS_ = false;
final isWindows_ = false;
final isMacOS_ = false;
final isLinux_ = false;
final isWeb_ = true;
final isWebDesktop_ = !js.context.callMethod('isMobile');

final isDesktop_ = false;

final _localOs = js.context.callMethod('getByName', ['local_os', '']);
final isWebOnWindows_ = _localOs == kPeerPlatformWindows;
final isWebOnLinux_ = _localOs == kPeerPlatformLinux;
final isWebOnMacOS_ = _localOs == kPeerPlatformMacOS;

/// web 无本机进程能力,SessionHost 拉起仅桌面原生支持。
Future<bool> launchSessionHost(String uid,
    {String kind = 'remote', String? name, String? token}) async {
  return false;
}
