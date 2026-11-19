# HPM5E31 固件

| 路径 | 内容 |
| --- | --- |
| `src/ecat.c` | 从站启动及 P0 MDIO 诊断 |
| `src/pdo_application.c` | GPIO、PDI 相位和诊断序号回读 |
| `port/` | ESC、以太网 PHY、EEPROM 仿真适配 |
| `boards/hpm5e00kit/` | 板级时钟、引脚及调试配置 |
| `phy-diag/` | 可独立加载的 RAM PHY 诊断 |

## 构建

需要 HPM SDK 1.11.0、适配 HPM5E31 的 GNU RISC-V 工具链、CMake、Ninja 和 SDK 的 Python 依赖。设置 `HPM_SDK_BASE` 和 `GNURISCV_TOOLCHAIN_PATH`。在仓库根目录执行以下 PowerShell 命令，并将路径替换为本机路径。

```powershell
$env:HPM_SDK_BASE = "C:/sdk/hpm_sdk"
$env:GNURISCV_TOOLCHAIN_PATH = "C:/toolchains/riscv"
$luckycat = "C:/deps/LuckyCAT_SoftWare"

cmake -S firmware/hpm5e31 -B build/hpm5e31-phase -G Ninja "-DLUCKYCAT_ROOT=$luckycat"
cmake --build build/hpm5e31-phase

cmake -S firmware/hpm5e31 -B build/hpm5e31-echo -G Ninja "-DLUCKYCAT_ROOT=$luckycat" -DHPM_PDO_MODE=echo
cmake --build build/hpm5e31-echo

cmake -S firmware/hpm5e31/phy-diag -B build/hpm5e31-phy -G Ninja
cmake --build build/hpm5e31-phy
```

`LUCKYCAT_ROOT` 默认指向以前的本地目录 `ecat-ram/LuckyCAT_SoftWare-main`。它提供 `demo/4_ethercat_io/ecat_io/SSC/Src`、生成的 `application/digital_io.c` 和 `demo/4_ethercat_io/OLED`。推荐使用 LuckyCAT_SoftWare 基线 `17017ac1c6a92959813a28e2f5072640d55b4f26`。正常构建时直接使用这里的应用逻辑。旧的 PDI 和回读补丁用于维护上游原目录工作流。 [LuckyCAT 上游](https://github.com/coinlockerbaby/LuckyCAT_SoftWare/tree/17017ac1c6a92959813a28e2f5072640d55b4f26) · [历史 PDI 补丁](../../patches/luckycat-p0-diagnostics.patch) · [历史回读补丁](../../patches/luckycat-pdo-echo.patch)

SDK 生成的镜像位于各构建目录的 `output/demo.elf`。

## PDO 和部署模式

| 构建模式 | RxPDO `0x7010:00` | TxPDO `0x6000:00` |
| --- | --- | --- |
| `phase`，默认 Flash XIP | 32 位输出，控制 LED 和 OLED 显示 | 低 16 位 GPIO，高 16 位 PDI 相位 µs，`0xffff` 表示无有效相位 |
| `echo`，默认 RAM | 30 位诊断序号，LED 保持关闭 | 位 0–1 为 GPIO，位 2–31 回显序号。零输出时返回标识 `0xECA74CA0`，最低两位仍为 GPIO |

回读模式优先通过 CMSIS-DAP 加载 ELF 到 RAM。回读测试结束后复位板卡以恢复 Flash 固件。 [主站回读脚本和测试口径](../../HostTools/rust-monitor/README.md#直连-pdo-回读测试)

板级和 ESC/PHY 源码基于上述 LuckyCAT 基线中的 HPMicro BSD-3-Clause 文件。`src/ecat.c` 加入了 P0 诊断，`src/pdo_application.c` 提供此仓库的 PDO 行为。Beckhoff SSC、生成的应用及对象字典由本地依赖提供。 [HPMicro 许可](LICENSE-HPMicro) · [Beckhoff SSC 许可](https://www.beckhoff.com/media/downloads/slave-stack-code/ethercat_ssc_license.pdf)
