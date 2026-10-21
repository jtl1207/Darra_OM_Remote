import 'package:flutter_hbb/common/widgets/address_book.dart';
import 'package:flutter_hbb/generated_bridge.dart';
import 'package:flutter_hbb/models/ab_model.dart';
import 'package:flutter_hbb/models/peer_model.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_test/flutter_test.dart';

/// 地址簿按 tag 分组单测: groupPeersByTag 纯分组 + 「按 tag 分组」开关读写假注入; 禁真网。

Peer _peer(String id, List<String> tags) =>
    Peer.fromJson({'id': id, 'alias': id, 'tags': tags});

/// 假 bind: 内存本地选项存取, 不碰原生与网络。
class _OptionBinding extends Fake implements RustdeskImpl {
  final store = <String, String>{};

  @override
  String mainGetLocalOption({required String key, dynamic hint}) =>
      store[key] ?? '';

  @override
  Future<void> mainSetLocalOption(
      {required String key, required String value, dynamic hint}) async {
    store[key] = value;
  }

  @override
  bool isCustomClient({dynamic hint}) => true;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('groupPeersByTag 分组', () {
    final peers = [
      _peer('1', []), // 无 tag
      _peer('2', ['a']),
      _peer('3', ['a', 'b']), // 只归首命中 a
      _peer('4', ['b']),
      _peer('5', []), // 空 tag 列表
    ];

    test('分组键: 首个 tag 归组, 组序按首个命中, 不重复成行', () {
      final g = groupPeersByTag(peers, []);
      expect(g.keys.toList(), [kUntagged, 'a', 'b']);
      expect(g['a']!.map((e) => e.id).toList(), ['2', '3']);
      expect(g['b']!.map((e) => e.id).toList(), ['4']); // 3 不再出现
    });

    test('kUntagged 归组: 无 tag 与空 tag 列表同进 Untagged', () {
      final g = groupPeersByTag(peers, []);
      expect(g[kUntagged]!.map((e) => e.id).toList(), ['1', '5']);
    });

    test('selectedTags 过滤: 未命中剔除', () {
      final g = groupPeersByTag(peers, ['b']);
      expect(g.keys.toList(), ['b']);
      expect(g['b']!.map((e) => e.id).toList(), ['3', '4']);
    });

    test('selectedTags 含 kUntagged: 只筛无 tag 设备', () {
      final g = groupPeersByTag(peers, [kUntagged]);
      expect(g.keys.toList(), [kUntagged]);
      expect(g[kUntagged]!.map((e) => e.id).toList(), ['1', '5']);
    });

    test('空 peers 与空 tag 列表筛选: 结果为空', () {
      expect(groupPeersByTag([], ['a']), isEmpty);
      expect(groupPeersByTag(peers, ['no-such-tag']), isEmpty);
    });

    test('sortByTag: 组名字典序', () {
      final g = groupPeersByTag(peers, [], sortByTag: true);
      expect(g.keys.toList(), [kUntagged, 'a', 'b']);
    });
  });

  group('「按 tag 分组」开关读写 (假注入本地选项)', () {
    late _OptionBinding binding;

    setUp(() {
      binding = _OptionBinding();
      platformFFI.setBindingForTesting(binding);
    });

    test('默认开: 未存选项时 shouldGroupAbTags 为真', () {
      expect(shouldGroupAbTags(), isTrue);
    });

    test('读态: 显式 N 为关, Y 与空值为开', () {
      binding.store[groupAbTagOption] = 'N';
      expect(shouldGroupAbTags(), isFalse);
      binding.store[groupAbTagOption] = 'Y';
      expect(shouldGroupAbTags(), isTrue);
      binding.store[groupAbTagOption] = '';
      expect(shouldGroupAbTags(), isTrue);
    });

    test('写关: 落 N + 同步回调, 读回为关', () async {
      final synced = <bool>[];
      await setGroupAbTagSwitch(false, onChanged: synced.add);
      expect(binding.store[groupAbTagOption], 'N');
      expect(synced, [false]);
      expect(shouldGroupAbTags(), isFalse);
    });

    test('写开: 落 Y + 同步回调, 往返读写一致', () async {
      final synced = <bool>[];
      await setGroupAbTagSwitch(true, onChanged: synced.add);
      expect(binding.store[groupAbTagOption], 'Y');
      expect(synced, [true]);
      expect(shouldGroupAbTags(), isTrue);
      await setGroupAbTagSwitch(false, onChanged: synced.add);
      expect(shouldGroupAbTags(), isFalse);
      await setGroupAbTagSwitch(true, onChanged: synced.add);
      expect(synced, [true, false, true]);
      expect(shouldGroupAbTags(), isTrue);
    });
  });
}
