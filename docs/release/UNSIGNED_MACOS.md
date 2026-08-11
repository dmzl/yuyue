# 屿阅 macOS Release / Yuyue macOS Release

这是一个未签名的 macOS 构建版本。它没有使用 Apple Developer ID 签名，也没有经过 Apple 公证。

This is an unsigned macOS build. It is not signed with an Apple Developer ID and is not notarized by Apple.

## 验证下载 / Verify the download

从本 Release 下载 `.dmg` 和 `SHA256SUMS` 文件，然后运行：

Download the `.dmg` and `SHA256SUMS` files from this release, then run:

```bash
shasum -a 256 -c SHA256SUMS
```

只有在下载的 DMG 校验结果显示 `OK` 后才继续。

Continue only when the checksum for the downloaded DMG reports `OK`.

## 在 macOS 上安装 / Install on macOS

1. 打开已验证的 DMG，将“屿阅”拖入 Applications。

   Open the verified DMG and drag Yuyue into Applications.
2. 首次打开“屿阅”。由于构建未签名，macOS 可能显示 Gatekeeper 警告。

   Open Yuyue once. macOS may show a Gatekeeper warning because this build is unsigned.
3. 如果 macOS 阻止应用，请打开 **系统设置 → 隐私与安全性**，查看被阻止的应用提示；确认校验和后才选择 **仍要打开**。

   If macOS blocks the app, open **System Settings → Privacy & Security**, review the blocked-app message, and choose **Open Anyway** only after verifying the checksum.
4. 确认应用名称和发布者提示后，从 Applications 打开应用。

   Confirm the application name and publisher prompt, then open it from Applications.

应用在本机读取 Markdown 文件。HTTPS 图片在当前标签显式允许前保持阻断；标签关闭后授权会被丢弃。

The application reads Markdown files locally. HTTPS images remain blocked until explicitly allowed for the current tab; the authorization is discarded when that tab closes.

GitHub Actions 会为标签对应的源代码 revision 生成构建来源证明。若要从源码构建，请参见仓库 README，并在 macOS 上运行 `npm ci`、`npm run tauri -- build --bundles app --ci --no-sign`，随后运行 `bash scripts/build-unsigned-dmg.sh`。DMG 步骤有意跳过 Finder 自动化，因此也可在无 GUI 的 CI 会话中运行。

GitHub Actions generates build provenance for the source revision associated with the tag. To build from source, see the repository README and run `npm ci`, `npm run tauri -- build --bundles app --ci --no-sign`, followed by `bash scripts/build-unsigned-dmg.sh` on macOS. The DMG step intentionally skips Finder automation so it also works in non-GUI CI sessions.

DMG 文件名架构来自打包 Mach-O 可执行文件（`arm64`、`x86_64` 或 `universal2`），而不是宿主进程架构。

The DMG filename architecture is derived from the bundled Mach-O executable (`arm64`, `x86_64`, or `universal2`), not from the host process architecture.
