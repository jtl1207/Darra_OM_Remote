# Darra_OM_Remote

Darra 远程控制端（基于 [rustdesk](https://github.com/rustdesk/rustdesk) 魔改）的源码公开备份仓。本仓内容为 Darra_OM 仓 `src/控制端/rustdesk/` 子树的历史镜像（git subtree split 同步），用于 AGPL 合规公开与防丢失。

改动清单详见 Darra_OM 仓 `内部文档/RustDesk魔改-架构方案.md`（内部文档），摘要：

- 品牌/图标/启动图替换为 Darra；外壳 UX（设备列表、最近会话、托盘、多窗口、主题、多语言）与上游一致
- 连接引擎替换为 Darra 自有远程会话栈：仅保留 Flutter 外壳，会话由独立进程承载，与闭源组件仅经进程协议耦合（不静态链接）
- 账号、设备列表、地址簿、tag 改接 Darra 统一登录与设备管理，tag/地址簿双向同步
- 移除未支持项：RustDesk 自有 ID/中继/打洞栈、自动更新、语音、隧道、USB 等

## 许可

[AGPL-3.0](LICENSE)，与上游相同。上游版权归 rustdesk 各贡献者，本仓保留全部原有版权与许可证头。以本仓源码提供网络服务时，须按 AGPL 向使用者公开对应完整源码。

上游原始 README 见 [README.rustdesk.md](README.rustdesk.md)。