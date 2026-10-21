import 'dart:convert';
import 'dart:io';

import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/models/om_auth.dart';

final isAndroid_ = Platform.isAndroid;
final isIOS_ = Platform.isIOS;
final isWindows_ = Platform.isWindows;
final isMacOS_ = Platform.isMacOS;
final isLinux_ = Platform.isLinux;
final isWeb_ = false;
final isWebDesktop_ = false;

final isDesktop_ = Platform.isWindows || Platform.isMacOS || Platform.isLinux;

final isWebOnWindows_ = false;
final isWebOnLinux_ = false;
final isWebOnMacOS_ = false;

/// 拉起会话组件 Darra远程会话.exe(SessionHost),stdin 写 START <json>。
/// 契约对齐 SessionHost/Program.cs ParseStart: uid/token 必填,kind ∈ remote|files|terminal。
/// token 只经 stdin 驻内存,不进 argv/标题/日志;查找顺序: 同目录 → C:\DARRA\OM_CONSOLE\。
Future<bool> launchSessionHost(String uid,
    {String kind = 'remote', String? name, String? token}) async {
  if (uid.isEmpty) return false;
  final token2 = token ?? OmAuth.instance.accessToken ?? '';
  if (token2.isEmpty) return false;
  final exeDir = File(Platform.resolvedExecutable).parent.path;
  String? exePath;
  for (final dir in kSessionHostSearchDirs) {
    final candidate =
        dir.isEmpty ? '$exeDir${Platform.pathSeparator}$kSessionHostExeName' : '$dir$kSessionHostExeName';
    if (File(candidate).existsSync()) {
      exePath = candidate;
      break;
    }
  }
  if (exePath == null) return false;
  final start = {
    'uid': uid,
    'token': token2,
    'kind': kind,
    if (name != null && name.isNotEmpty) 'name': name,
  };
  final process = await Process.start(exePath, const []);
  process.stdin.writeln('$kSessionHostStartPrefix${jsonEncode(start)}');
  await process.stdin.close();
  return true;
}
