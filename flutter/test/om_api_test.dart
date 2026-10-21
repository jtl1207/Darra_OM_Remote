import 'dart:async';
import 'dart:convert';

import 'package:flutter_hbb/models/om_api.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

/// om_api 单测: 设备解析 / tags 解析 / 404 缺口 / 401 / HTTP 状态, 全程 MockClient 不发真网。

http.Client _client(Future<http.Response> Function(http.Request req) handler) =>
    MockClient(handler);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('设备解析', () {
    test('顶层数组: uid/名称/在线态/tags 全解析 (UTF-8 中文不乱码)', () async {
      final api = OmApi(
        httpClient: _client((req) async {
          expect(req.url.toString(), 'https://om.darra.xyz/api/om/devices');
          expect(req.headers['authorization'], 'Bearer tok-1');
          // 真实母服常见形态: JSON UTF-8 但 content-type 不带 charset
          return http.Response.bytes(
              utf8.encode(jsonEncode([
                {
                  'uid': 'u1',
                  'custom_name': '车床',
                  'online': true,
                  'tags': ['a', 'b'],
                },
                {'uid': 'u2', 'name': '铣床', 'status': 'active', 'tags': ['b']},
                {'uid': 'u3', 'status': 'offline', 'tags': []},
              ])),
              200);
        }),
      );
      final devices = await api.listDevices('tok-1');
      expect(devices, hasLength(3));
      expect(devices[0].uid, 'u1');
      expect(devices[0].name, '车床');
      expect(devices[0].online, isTrue);
      expect(devices[0].tags, ['a', 'b']);
      expect(devices[1].online, isTrue); // status=active
      expect(devices[2].online, isFalse);
      expect(devices[2].name, 'u3'); // custom_name/name 缺失回落 uid
    });

    test('data / devices 包裹形式都能解析', () async {
      for (final wrapper in ['data', 'devices']) {
        final api = OmApi(
          httpClient: _client((req) async {
            return http.Response(
                jsonEncode({
                  wrapper: [
                    {'uid': 'u9', 'name': 'n9'}
                  ]
                }),
                200);
          }),
        );
        final devices = await api.listDevices('t');
        expect(devices, hasLength(1), reason: 'wrapper=$wrapper');
        expect(devices.single.uid, 'u9');
      }
    });
  });

  group('tags 解析', () {
    test('去空去重保序', () async {
      final api = OmApi(
        httpClient: _client((req) async {
          return http.Response(
              jsonEncode([
                {
                  'uid': 'u1',
                  'name': 'n',
                  'tags': [' b ', 'a', '', 'b', null],
                }
              ]),
              200);
        }),
      );
      final devices = await api.listDevices('t');
      expect(devices.single.tags, ['b', 'a']);
    });

    test('tags 缺失/非法形态回落空列表', () async {
      final api = OmApi(
        httpClient: _client((req) async {
          return http.Response(
              jsonEncode([
                {'uid': 'u1', 'name': 'n'},
                {'uid': 'u2', 'name': 'n', 'tags': 'bad'},
                {'uid': 'u3', 'name': 'n', 'tags': null},
              ]),
              200);
        }),
      );
      final devices = await api.listDevices('t');
      for (final d in devices) {
        expect(d.tags, isEmpty, reason: 'uid=${d.uid}');
      }
    });

    test('patchDeviceTags: 去空去重后整组替换 PATCH', () async {
      http.Request? seen;
      final api = OmApi(
        httpClient: _client((req) async {
          seen = req;
          return http.Response('', 200);
        }),
      );
      await api.patchDeviceTags('tok-2', 'u 1', [' x ', 'y', 'x', '']);
      expect(seen, isNotNull);
      expect(seen!.method, 'PATCH');
      expect(seen!.url.toString(),
          'https://om.darra.xyz/api/om/devices/u%201/tags');
      expect(jsonDecode(seen!.body), {'tags': ['x', 'y']});
      expect(seen!.headers['authorization'], 'Bearer tok-2');
    });
  });

  group('HTTP 状态', () {
    test('404 缺口: isMissingEndpoint=true', () async {
      final api = OmApi(
        httpClient: _client((req) async => http.Response('no endpoint', 404)),
      );
      try {
        await api.listDevices('t');
        fail('应当抛 OmApiException');
      } on OmApiException catch (e) {
        expect(e.statusCode, 404);
        expect(e.isMissingEndpoint, isTrue);
        expect(e.bodySnippet, 'no endpoint');
      }
    });

    test('401: 抛异常且回登录页回调', () async {
      var unauthorized = 0;
      final api = OmApi(
        httpClient: _client((req) async => http.Response('expired', 401)),
        onUnauthorized: (who) async {
          unauthorized += 1;
          expect(who, 'listDevices');
        },
      );
      await expectLater(api.listDevices('t'), throwsA(isA<OmApiException>()));
      await Future.delayed(Duration.zero);
      expect(unauthorized, 1);
    });

    test('401 走 patchDeviceTags 同样触发 401 处理', () async {
      var unauthorized = 0;
      final api = OmApi(
        httpClient: _client((req) async => http.Response('', 401)),
        onUnauthorized: (who) async {
          unauthorized += 1;
          expect(who, 'patchDeviceTags');
        },
      );
      await expectLater(
          api.patchDeviceTags('t', 'u1', ['a']), throwsA(isA<OmApiException>()));
      await Future.delayed(Duration.zero);
      expect(unauthorized, 1);
    });

    test('500: 状态码如实带出, isMissingEndpoint=false', () async {
      final api = OmApi(
        httpClient: _client((req) async => http.Response('boom', 500)),
      );
      try {
        await api.listDevices('t');
        fail('应当抛 OmApiException');
      } on OmApiException catch (e) {
        expect(e.statusCode, 500);
        expect(e.isMissingEndpoint, isFalse);
        expect(e.message, 'listDevices 失败');
      }
    });

    test('非 JSON 响应: 状态码 -1 响应非 JSON', () async {
      final api = OmApi(
        httpClient: _client((req) async => http.Response('<html>', 200)),
      );
      try {
        await api.listDevices('t');
        fail('应当抛 OmApiException');
      } on OmApiException catch (e) {
        expect(e.statusCode, -1);
        expect(e.message, 'listDevices 响应非 JSON');
      }
    });
  });

  test('SSE 连接可注入: 订阅走注入事件流', () async {
    final controller = StreamController<OmSseEvent>();
    final api = OmApi(sseFactory: (_) => controller.stream);
    final events = <OmSseEvent>[];
    final sub = api.subscribeEvents('tok').listen(events.add);
    controller.add(OmSseEvent('device-state', {'uid': 'u1', 'online': true}));
    await pumpEventQueue();
    expect(events, hasLength(1));
    expect(events.single.event, 'device-state');
    expect(events.single.data['uid'], 'u1');
    await sub.cancel();
    await controller.close();
  });
}
