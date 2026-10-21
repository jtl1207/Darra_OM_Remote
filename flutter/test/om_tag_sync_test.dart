import 'dart:async';
import 'dart:convert';

import 'package:flutter_hbb/models/om_api.dart';
import 'package:flutter_hbb/models/om_tag_sync.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// om_tag_sync 单测: 乐观更新 / 失败回滚 / 回声去重 / revision 比大小 / 重试 flush,
/// 网络与计时全部注入, 不发真网。

TagSync _sync({
  required http.Client client,
  String token = 'tok',
  Duration retryInterval = const Duration(milliseconds: 20),
  DateTime Function()? clock,
}) =>
    TagSync.forTest(
      api: OmApi(httpClient: client),
      tokenProvider: () => token,
      retryInterval: retryInterval,
      clock: clock,
    );

OmDeviceData _dev(String uid, List<String> tags, {int? revision}) =>
    OmDeviceData(
      uid: uid,
      name: uid,
      online: true,
      tags: tags,
      raw: {if (revision != null) 'revision': revision},
    );

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('乐观更新: 立即落内存态并整组替换 PATCH', () async {
    http.Request? seen;
    final sync = _sync(
      client: MockClient((req) async {
        seen = req;
        return http.Response('', 200);
      }),
    );
    final ok = await sync.setTags('u1', [' b ', 'a', 'a', '']);
    expect(ok, isTrue);
    expect(sync.tagsByUid['u1'], ['b', 'a']); // 去空去重保序
    expect(sync.allTags, ['a', 'b']);
    expect(seen, isNotNull);
    expect(seen!.method, 'PATCH');
    expect(jsonDecode(seen!.body), {'tags': ['b', 'a']});
    sync.reset();
  });

  test('失败回滚: 服务端拒绝恢复旧值', () async {
    var fail = false;
    final sync = _sync(
      client: MockClient((req) async {
        if (fail) return http.Response('', 500);
        return http.Response('', 200);
      }),
    );
    expect(await sync.setTags('u1', ['a']), isTrue);
    fail = true;
    expect(await sync.setTags('u1', ['b']), isFalse);
    expect(sync.tagsByUid['u1'], ['a']); // 回滚到旧值
    expect(sync.allTags, ['a']);
    sync.reset();
  });

  test('失败回滚: 404 缺端点同样恢复旧值', () async {
    var fail = false;
    final sync = _sync(
      client: MockClient((req) async {
        if (fail) return http.Response('', 404);
        return http.Response('', 200);
      }),
    );
    expect(await sync.setTags('u1', ['a']), isTrue);
    fail = true;
    expect(await sync.setTags('u1', ['b']), isFalse);
    expect(sync.tagsByUid['u1'], ['a']);
    sync.reset();
  });

  test('回声去重: 本机写回同值回声不覆盖较新的本地状态', () async {
    var patchCount = 0;
    final pending = Completer<http.Response>();
    final sync = _sync(
      client: MockClient((req) async {
        patchCount += 1;
        if (patchCount == 1) return http.Response('', 200);
        return pending.future; // 第二次 PATCH 挂起, 模拟云端回声先到
      }),
      clock: () => DateTime.fromMillisecondsSinceEpoch(1700000000000, isUtc: true),
    );
    expect(await sync.setTags('u1', ['x']), isTrue); // 记回声指纹 x
    final second = sync.setTags('u1', ['y']); // 乐观更新 y, PATCH 挂起
    expect(sync.tagsByUid['u1'], ['y']);
    // 云端回声: 与首次写回同值 (revision 新), 不得覆盖本地 y
    sync.absorbFromDevices([_dev('u1', ['x'], revision: 5)]);
    expect(sync.tagsByUid['u1'], ['y']);
    pending.complete(http.Response('', 200));
    expect(await second, isTrue);
    expect(sync.tagsByUid['u1'], ['y']);
    // 非回声的远端新值照常落地
    sync.absorbFromDevices([_dev('u1', ['z'], revision: 6)]);
    expect(sync.tagsByUid['u1'], ['z']);
    sync.reset();
  });

  test('revision 比大小: 旧 revision 跳过', () async {
    final sync = _sync(
      client: MockClient((req) async => http.Response('', 200)),
    );
    sync.absorbFromDevices([_dev('u1', ['a'], revision: 5)]);
    expect(sync.tagsByUid['u1'], ['a']);
    sync.absorbFromDevices([_dev('u1', ['b'], revision: 3)]);
    expect(sync.tagsByUid['u1'], ['a']); // 旧 revision 不回退
    sync.absorbFromDevices([_dev('u1', ['c'], revision: 6)]);
    expect(sync.tagsByUid['u1'], ['c']);
    sync.reset();
  });

  test('重试 flush: 网络异常入队, 手动冲刷成功后落内存态', () async {
    var fail = true;
    var patchCount = 0;
    final sync = _sync(
      client: MockClient((req) async {
        patchCount += 1;
        if (fail) throw Exception('net down');
        return http.Response('', 200);
      }),
    );
    expect(await sync.setTags('u1', ['x']), isFalse);
    expect(sync.tagsByUid.containsKey('u1'), isFalse); // 回滚
    fail = false;
    await sync.flushRetry();
    expect(sync.tagsByUid['u1'], ['x']); // 重试成功落内存态
    expect(patchCount, 2);
    sync.reset();
  });

  test('重试 flush: 离线重试 Timer 按注入周期自动冲刷', () async {
    var fail = true;
    final sync = _sync(
      retryInterval: const Duration(milliseconds: 20),
      client: MockClient((req) async {
        if (fail) throw Exception('net down');
        return http.Response('', 200);
      }),
    );
    expect(await sync.setTags('u1', ['x']), isFalse);
    fail = false;
    await Future.delayed(const Duration(milliseconds: 300));
    expect(sync.tagsByUid['u1'], ['x']); // 定时器已冲刷
    sync.reset();
  });

  test('无登录令牌: 乐观值回滚并进重试队列', () async {
    var patchCount = 0;
    final sync = _sync(
      token: '',
      client: MockClient((req) async {
        patchCount += 1;
        return http.Response('', 200);
      }),
    );
    expect(await sync.setTags('u1', ['x']), isFalse);
    expect(sync.tagsByUid.containsKey('u1'), isFalse);
    expect(patchCount, 0); // 令牌为空不发请求
    sync.reset();
  });

  test('SSE 订阅: device-state / device-tags 事件吸收', () async {
    final controller = StreamController<OmSseEvent>();
    final sync = TagSync.forTest(
      api: OmApi(sseFactory: (_) => controller.stream),
      tokenProvider: () => 'tok',
    );
    final states = <String, bool>{};
    sync.onDeviceState = (uid, online) => states[uid] = online;
    sync.startSse('tok');
    controller.add(OmSseEvent('device-state', {'uid': 'u1', 'online': true}));
    controller.add(OmSseEvent('device-tags', {
      'uid': 'u1',
      'tags': ['a'],
      'revision': 2,
    }));
    await pumpEventQueue();
    expect(states['u1'], isTrue);
    expect(sync.tagsByUid['u1'], ['a']);
    sync.stopSse();
    await controller.close();
    sync.reset();
  });
}
