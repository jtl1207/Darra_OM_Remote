import 'package:flutter_hbb/models/ab_model.dart';
import 'package:flutter_hbb/models/om_api.dart';
import 'package:flutter_hbb/models/peer_model.dart';
import 'package:flutter_test/flutter_test.dart';

/// ab_model 单测: OM 设备→Peer 映射 / 按 tag 分组筛选 / kUntagged 语义。

Peer _peer(String id, List<String> tags) =>
    Peer.fromJson({'id': id, 'alias': id, 'tags': tags});

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('Om 设备→Peer 映射', () {
    test('uid/名称/在线态/tags 全量落到 Peer', () {
      final d = OmDeviceData(
        uid: 'u1',
        name: '车床',
        online: true,
        tags: ['a', 'b'],
        raw: {'uid': 'u1', 'revision': 7},
      );
      final p = peerFromOmDevice(d);
      expect(p.id, 'u1');
      expect(p.alias, '车床');
      expect(p.online, isTrue);
      expect(p.tags, ['a', 'b']);
    });

    test('离线设备与空 tags 映射', () {
      final d = OmDeviceData(
        uid: 'u2',
        name: 'u2',
        online: false,
        tags: const [],
        raw: const {'uid': 'u2'},
      );
      final p = peerFromOmDevice(d);
      expect(p.id, 'u2');
      expect(p.online, isFalse);
      expect(p.tags, isEmpty);
    });
  });

  group('按 tag 分组筛选', () {
    final peers = [
      _peer('1', []), // 无 tag
      _peer('2', ['a']),
      _peer('3', ['a', 'b']),
      _peer('4', ['b']),
    ];

    test('未选 tag: 按首个 tag 分组, 无 tag 归 kUntagged, 每台只归首命中组', () {
      final groups = groupPeersByTag(peers, []);
      expect(groups.keys.toList(), [kUntagged, 'a', 'b']);
      expect(groups[kUntagged]!.map((e) => e.id).toList(), ['1']);
      expect(groups['a']!.map((e) => e.id).toList(), ['2', '3']);
      expect(groups['b']!.map((e) => e.id).toList(), ['4']); // 3 不重复出现
    });

    test('选中 tag 过滤: 未命中设备剔除', () {
      final groups = groupPeersByTag(peers, ['b']);
      expect(groups.keys.toList(), ['b']);
      expect(groups['b']!.map((e) => e.id).toList(), ['3', '4']);
    });

    test('kUntagged 参与筛选: 无 tag 设备命中 kUntagged 组', () {
      final groups = groupPeersByTag(peers, ['b', kUntagged]);
      expect(groups.keys.toList(), [kUntagged, 'b']);
      expect(groups[kUntagged]!.map((e) => e.id).toList(), ['1']);
      expect(groups['b']!.map((e) => e.id).toList(), ['3', '4']);
    });

    test('kUntagged 不命中有 tag 设备', () {
      final groups = groupPeersByTag(peers, [kUntagged]);
      expect(groups.keys.toList(), [kUntagged]);
      expect(groups[kUntagged]!.map((e) => e.id).toList(), ['1']);
    });

    test('sortByTag: 组名排序', () {
      final groups = groupPeersByTag(peers, [], sortByTag: true);
      expect(groups.keys.toList(), [kUntagged, 'a', 'b']);
    });
  });

  test('kUntagged 常量', () {
    expect(kUntagged, 'Untagged');
    expect(groupAbTagOption, 'group-ab-by-tag');
  });
}
