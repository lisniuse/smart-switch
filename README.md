<p align="center">
  <img src="assets/logo.svg" width="112" height="112" alt="Smart Switch 标志">
</p>

<h1 align="center">Smart Switch</h1>

Windows 10 和 11 的 Rust 托盘工具。当前台窗口进入全屏或无边框全屏，或切回仍处于全屏的游戏时，自动选择 **英语（美国）— 美式键盘**。普通窗口中的可编辑输入框获得焦点时，自动选择 **豆包输入法**。

## 使用

1. 按 **Win + 空格**，确认列表里有 **英语（美国）— 美式键盘**和**豆包输入法**。可在 Windows 的**设置 → 时间和语言 → 语言和区域**管理键盘。[Microsoft 键盘布局说明](https://support.microsoft.com/en-us/windows/hardware/input-devices/manage-the-language-and-keyboard-input-layout-settings-in-windows)
2. 双击 `smart-switch.exe`。程序驻留系统托盘，图标可能收在任务栏右下角的 **^** 菜单里。
3. 右键托盘图标可暂停、恢复，设置开机启动，打开 Windows 语言设置或退出。悬停可查看切换状态。

勾选**开机启动**后，Smart Switch 会在当前用户登录 Windows 时运行。默认关闭，可随时取消勾选。移动程序后，在新位置运行并重新勾选即可更新路径。

带标题栏的普通最大化窗口也适用输入框规则。程序通过系统控件或 Windows UI Automation 识别可编辑输入框，部分应用的自绘控件可能无法识别。[Microsoft UI Automation 文档](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation-getfocusedelement)

每次进入全屏或获得输入焦点时触发一次切换，你仍可按 **Win + 空格** 手动选择布局。个别游戏可能拒绝 Windows 的键盘布局切换请求，托盘提示会显示切换失败。[Windows 切换请求说明](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-inputlangchangerequest)

## 从源码构建

使用 Windows 上的 Rust stable 和 MSVC 工具链。请安装 Visual Studio Build Tools 的 C++ 桌面开发组件及 Windows SDK，以编译 EXE 图标资源。

```powershell
cargo build --locked --release
```

程序位于 `target/release/smart-switch.exe`，可以单独复制运行。

## 许可证

[MIT](LICENSE)
