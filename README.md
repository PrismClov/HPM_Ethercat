# HPM5E31-KIT EtherCAT

| 目录 | 内容 |
| --- | --- |
| [`firmware/hpm5e31/`](firmware/hpm5e31/README.md) | HPM5E31 从站、引脚配置、ESC/PHY 适配及 RAM PHY 诊断 |
| [`HostTools/`](HostTools/README.md) | 主站、桌面监测器和现有 MC02 工具 |
| [`tools/hpm5e31/`](tools/hpm5e31/README.md) | HPM5E31 联调、PDO 探测和 WSL 测试脚本 |
| [`docs/hardware/`](docs/hardware/p0-probe-points.svg) | P0 测点图 |
| [`docs/benchmarks/hpm5e31/`](docs/benchmarks/hpm5e31/README.md) | 原始扫频和回读数据 |
| `patches/` | 供历史上游版本使用的补丁 |

从站固件提供默认的 PDI 相位模式、RAM PDO 回读模式和独立的 RAM PHY 诊断。构建需要 HPM SDK 1.11.0、RISC-V 工具链和本地 LuckyCAT SSC 依赖，详细命令见 [固件构建说明](firmware/hpm5e31/README.md)。

主站可选 [Rust 监测器](HostTools/rust-monitor/README.md) 或 [Linux SOEM 命令行主站](HostTools/soem_master/README.md)。`HostTools/` 的 MC02 工具导入自 `141bfed`，HPM PDO 解码和 DC 启动适配的历史补丁可用于 Ethercat_MC02 `7df9d4fc6ff119eeb201bab16b6386cef0120ece`。见 [MC02 主站补丁](patches/linux-master-hpm-pdo.patch)。

已有的 `ecat-ram/` 本地依赖、工具链、构建目录和 Flash 备份可继续使用。新的构建建议输出到根目录的 `build/`。
