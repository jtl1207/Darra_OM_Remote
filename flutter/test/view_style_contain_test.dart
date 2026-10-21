import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_hbb/models/model.dart';

// 钉死远程画面等比 contain 缩放语义: adaptive 取宽高比最小值, 不拉伸不裁切。
void main() {
  test('adaptive 缩放 = 等比 contain (取两轴比值的较小者)', () {
    // 横屏视口 800x360, 远程 1920x1080: 高度先贴边 → s = 360/1080
    final landscape = ViewStyle(
        style: 'adaptive',
        width: 800,
        height: 360,
        displayWidth: 1920,
        displayHeight: 1080);
    expect(landscape.scale, closeTo(360 / 1080, 1e-9));

    // 竖屏视口 360x800, 远程 1920x1080: 宽度先贴边 → s = 360/1920
    final portrait = ViewStyle(
        style: 'adaptive',
        width: 360,
        height: 800,
        displayWidth: 1920,
        displayHeight: 1080);
    expect(portrait.scale, closeTo(360 / 1920, 1e-9));
  });

  test('original 缩放恒 1.0; adaptive 视口更大时等比放大贴边', () {
    final original = ViewStyle(
        style: 'original',
        width: 800,
        height: 360,
        displayWidth: 1920,
        displayHeight: 1080);
    expect(original.scale, 1.0);

    // 视口大于远程画面: adaptive 同样等比放大到贴边 (contain 语义, 不裁切)
    final big = ViewStyle(
        style: 'adaptive',
        width: 4000,
        height: 3000,
        displayWidth: 1920,
        displayHeight: 1080);
    expect(big.scale, closeTo(4000 / 1920, 1e-9));
  });

  test('resolveStyle 保留显式设置, 空值桌面端保持现状', () {
    expect(ViewStyle.resolveStyle('original'), 'original');
    expect(ViewStyle.resolveStyle('custom'), 'custom');
    expect(ViewStyle.resolveStyle('adaptive'), 'adaptive');
    // 单测跑在桌面环境: 空值维持现状(不改桌面行为)
    expect(ViewStyle.resolveStyle(''), '');
    expect(ViewStyle.resolveStyle(null), '');
  });
}
