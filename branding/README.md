# Darra OM 魔改 RustDesk — 品牌图标资源包

换皮不换骨：版式、尺寸、文件名全部对齐 `参考项目/rustdesk` 现行惯例；皮 = Darra 品牌标（黑圆角底 `#161616` + 白 D + 红点 `#cf000f`）。
源出处：`src/控制端/web/public/icon.svg`（与商城/官网/EtherCAT 文档站同源），留档于 `svg/darra-mark.svg`；单色变体 `svg/darra-mark-mono.svg`。
应用名：**Darra 远程**。再生产：`python tools/gen_icons.py`（纯 PIL，无外部依赖）。

## 本包布局 → 魔改树落点

| 本包路径 | 魔改树落点 (拷贝目标) | 说明 |
|---|---|---|
| `windows/app.ico` | `res/icon.ico`（`build.rs` `res.set_icon` 同源）；MSI 另拷 `res/msi/Package/Resources/icon.ico` | 多尺寸 16/24/32/48/64/128/256 |
| `windows/tray.ico` | `res/tray-icon.ico` | 托盘 16/32 |
| `common/icon.png` | `res/icon.png` | 1024 母版 |
| `common/32x32.png` `64x64.png` `128x128.png` `128x128@2x.png` | `res/` 同名 | 桌面/系统图标降采样集（256=@2x） |
| `common/icon-512.png` | 通用应用图标（商店/文档/PWA 等） | 512 PNG |
| `common/logo.png` | `flutter_assets/assets/logo.png` | 关于页 logo，深色字（浅底用） |
| `common/logo_light.png` | `flutter_assets/assets/logo_light.png` | 浅色主题变体（同 logo.png 深色字） |
| `common/logo_dark.png` | `flutter_assets/assets/logo_dark.png` | 深色主题变体（白字） |
| `common/splash.png` | 启动图（各端 splash 画布取中间 logo 或整图） | 1280x720 品牌黑底 + 标 + 「Darra 远程」 |
| `svg/darra-mark.svg` | `flutter/assets/icon.svg`（应用内矢量图标同源替换） | 品牌标 SVG 源 |
| `svg/darra-mark-mono.svg` | 单色/主题化备用源 | 白形体透明底 |
| `android/mipmap-*/ic_launcher.png` | `flutter/android/app/src/main/res/mipmap-*/ic_launcher.png` | 48/72/96/144/192 |
| `android/mipmap-*/ic_launcher_round.png` | 同目录 `ic_launcher_round.png` | 圆形裁切同尺寸 |
| `android/mipmap-*/ic_launcher_foreground.png` | 同目录 `ic_launcher_foreground.png` | 自适应前景 108/162/216/324/432 |
| `android/mipmap-*/ic_stat_logo.png` | 同目录 `ic_stat_logo.png` | 通知栏状态 24/36/48/72/96，LA 模式 |
| `android/mipmap-*/ic_launcher_monochrome.png` | 同目录 `ic_launcher_monochrome.png` | 主题化图标单色前景（比 rustdesk 多的专用件，XML 已指向它） |
| `android/mipmap-anydpi-v26/ic_launcher.xml` `ic_launcher_round.xml` | `res/mipmap-anydpi-v26/` 同名 | 自适应图标定义，`monochrome` 指 `ic_launcher_monochrome` |
| `android/values/ic_launcher_background.xml` | `res/values/`（并入现有 colors/values） | 自适应背景色 `#161616` |
| `android/values/strings.xml` | 并入 `res/values/strings.xml` 的 `app_name` | `<string name="app_name">Darra 远程</string>` |

## 尺寸对照（与 rustdesk 实测一致）

- `ic_launcher*`：mdpi 48 / hdpi 72 / xhdpi 96 / xxhdpi 144 / xxxhdpi 192
- `ic_launcher_foreground*`：108 / 162 / 216 / 324 / 432（108dp 自适应画布）
- `ic_stat_logo`：24 / 36 / 48 / 72 / 96（LA）
- Windows `app.ico`：16–256 全档；`tray.ico`：16/32

## 已知命名差异（仅两处，已按语义收敛）

1. `app.ico` / `tray.ico` 为本包工作名（用户规格），拷入魔改树时按上表改回 `res/icon.ico` / `res/tray-icon.ico`。
2. `ic_launcher_monochrome.png` 为本包新增件：Darra 标含红点，主题化图标（monochrome）不能复用彩色前景，故单独提供单色前景并在 XML 中引用；其余文件名与 rustdesk 逐一相同。
