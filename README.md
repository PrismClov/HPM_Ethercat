# HPM5E31-KIT EtherCAT 调试

这个仓库保存 IPB-TINY 板卡的 P0 测点图、PHY 诊断和 PDO 测试脚本，以及实时性测试所用的固件和 Linux 主站补丁。Flash 备份、下载的工具链和编译产物保留在本机，Git 忽略这些文件。

- [EtherCAT 上位机](HostTools/README.md)
- [P0 测点图](p0-probe-points.svg)
- [RAM PHY 诊断](ecat-ram/phy_diag.c)
- [Windows 发帧结果](ecat-ram/hpm_p0_timing.csv)

`HostTools/` 导入了 MC02 上位机的 `141bfed` 版本，并合入 HPM PDO 解码和 DC 启动适配。WSL 构建脚本直接使用仓库内的 C 版主站源码。

## 源码基线

| 源码 | 基线 | 补丁 |
|---|---|---|
| [LuckyCAT_SoftWare](https://github.com/coinlockerbaby/LuckyCAT_SoftWare) | `17017ac1c6a92959813a28e2f5072640d55b4f26` | [P0 诊断固件](patches/luckycat-p0-diagnostics.patch) |
| [Ethercat_MC02](https://github.com/PrismClov/Ethercat_MC02) | `7df9d4fc6ff119eeb201bab16b6386cef0120ece` | [HPM PDO 解码和 DC 启动](patches/linux-master-hpm-pdo.patch) |

在各自源码根目录用 `git apply` 应用相应补丁。固件使用 HPM SDK 1.11.0 和 `hpm5e00kit` 板级配置。RAM 诊断由 `ecat-ram/CMakeLists.txt` 构建，Flash 版从 LuckyCAT 的 `demo/4_ethercat_io/ecat_io` 构建。Flash 版用输入 PDO 的高 16 位上报 PDI 相位，单位为微秒，`0xffff` 表示无有效样本。

WSL 脚本沿用当前电脑的挂载路径、Realtek 网卡标识和软件目录。换机器时需要调整这些值。Linux 主站可使用 SOEM 2.0.0 源码构建，WSL 转发只在测试期间通过本机回环传输 EtherCAT 帧。
