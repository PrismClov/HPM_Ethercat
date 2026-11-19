# EtherCAT 总线监测器

Rust + egui 桌面程序。可切换 IgH、SOEM 和演示数据源，查看总线响应、从站位置、名称、厂商 ID、产品代码、修订号、AL 状态和错误标志。SOEM 还显示 AL 状态码。状态监测的刷新间隔可设为 100–5000 ms。勾选「HPM PDO」可采集并绘制 HPM5E31-KIT 的输入过程数据。

| 后端 | 平台 | 连接参数 | 行为 |
| --- | --- | --- | --- |
| IgH | Linux | 主站编号，例如 `0` | 打开现有主站，读取内核主站信息和当前从站状态。 |
| SOEM 2.0.0 | Linux / Windows | Linux 网卡名或 Windows Npcap 设备名 | 连接时初始化和扫描总线，随后读 AL 状态寄存器。 |
| 演示 | Linux / Windows | 无 | 模拟三台从站，其中一台周期性显示错误。 |

SOEM 扫描会将从站切到 INIT，并重置部分 ESC 配置和分配站地址。操作前停机，并确保该 EtherCAT 段没有其他主站运行。在状态监测模式下，扫描后保持 INIT。同一段总线更换设备后可断开重连重新识别。IgH 模式可用于观察已经运行的 IgH 主站，AL 状态码列中 `—` 表示接口未提供该字段。参见 [IgH 应用接口](https://docs.etherlab.org/ethercat/1.6/doxygen/group__ApplicationInterface.html) 和 [SOEM 2.0.0 源码](https://github.com/OpenEtherCATsociety/SOEM/tree/v2.0.0)。

## HPM5E31-KIT PDO 采集

1. 将单台 HPM5E31-KIT 直连主站网卡，运行仓库中的 P0/PDI 相位诊断固件。程序按厂商 ID `0x00024121`、产品代码 `0x00000001` 以及 CoE PDO 分配和映射校验设备。
2. 选择 IgH 或 SOEM，输入主站编号或网卡，勾选「HPM PDO」。停止该总线上的其他主站应用，确认后连接。IgH 模式会独占内核主站并创建 PDO 域，SOEM 模式会初始化总线并请求 SAFE-OP、OP。两种模式均固定发送 `0x7010:00 = 0`。
3. 查看原始输入、WKC、GPIO 位和 PDI 相位。点击 GPIO 位即可切换对应的阶梯曲线。演示模式也支持 PDO 曲线。

| 输入对象 | 字段 | 含义 |
| --- | --- | --- |
| `0x6000:00`，32 位小端 | 位 0–15 | GPIO 输入位图。仓库固件将 IN1 和 IN2 映射到位 0 和位 1。 |
| 同上 | 位 16–31 | PDI 相位，单位 µs。`0xffff` 表示该字段无有效样本。 |

曲线显示最近 10 秒。无有效 PDO 的周期在两张图上留空，`0xffff` 仅使相位曲线留空。SOEM 显示实际和期望 WKC，IgH 使用 PDO 域的完整性状态决定样本是否有效。断开后界面保留上一次记录。正常断开时，SOEM 请求 SAFE-OP，IgH 释放主站和域。

目标采集周期为 100–20000 µs，默认 5000 µs。采集使用固定周期，跳过已错过的时隙。2000 µs 及更短周期使用忙等减少唤醒抖动，会占用 CPU。实际周期受操作系统调度和报文往返影响，曲线按软件收到样本的单调时钟绘制。此版本使用非 DC 采集。当前机型适配针对单台板卡和 [仓库里的 PDI 相位固件](../../firmware/hpm5e31/README.md)，其他固件即使 PDO 大小相同也可能采用不同的高 16 位语义。

## 频率和丢帧测试

`pdo_bench` 使用桌面程序相同的 PDO 工作线程及后端协议，等待 OP 和有效 PDO 后计时，每个频点重新连接。测试也会独占总线，并持续发送零值输出。

```bash
ECAT_MONITOR_BACKEND_DIR="$(realpath build/native)" \
  cargo run --release --locked --no-default-features --example pdo_bench -- igh 0 15 5000 2000 1000 500
```

Windows 将 `ECAT_MONITOR_BACKEND_DIR` 指向后端所在目录，把末尾的 `igh 0` 换成 `soem` 和 Npcap 设备名。以微秒填写周期。输出为 CSV，`invalid_pct` 是已完成采样中 PDO 无效的比例，`missed_slots_pct` 是相对目标频率未完成的时隙比例。间隔分位数以 Rust 收到相邻样本的时间差计算。

### 直连 PDO 回读测试

先用原有 PDI 相位固件在原生 Linux 和物理直连网口上完成上述扫频。回读使用独立的诊断固件。使用 [HPM 固件工程](../../firmware/hpm5e31/README.md) 的 `HPM_PDO_MODE=echo` 构建并加载诊断固件。诊断固件将输出 PDO 中的测试序号放到输入 PDO 的位 2–31，位 0–1 仍为 GPIO。诊断序号不驱动 LED 或 OLED。优先用 RAM 构建加载诊断固件，测试完成后复位回到原有 PDI 相位固件。

```bash
python3 examples/pdo_echo_bench.py ./build/native/ecat-monitor-igh 0 15 5000 2000 1000 750 500 350 250
```

IgH 和 SOEM 均在读取到诊断固件签名前保持零输出。SOEM 使用同一脚本，传入 `ecat-monitor-soem` 或 `ecat-monitor-soem.exe` 以及直连网卡名或 Npcap 设备名。延迟从主机提交输出帧前到主机接收匹配序号后计算，包含软件调度和从站回显。`period_error` 是实测接收间隔与目标周期之差的绝对值。CSV 分开报告无效 PDO、错过的目标时隙和发出后未收到对应回显的序号。回读模式衡量诊断环路的频率和时延。

[实机扫频、回读结果及原始 CSV](../../docs/benchmarks/hpm5e31/README.md)。

## Linux 构建与运行

需要 Rust stable、C 编译器、CMake 3.28+ 和支持 OpenGL 的桌面。Ubuntu 上可安装 `build-essential libgl1-mesa-dev libxkbcommon-dev fonts-wqy-zenhei`。使用其他中文字体时，可通过 `ECAT_MONITOR_FONT` 指向字体文件。

在本目录执行：

```bash
cargo build --release --locked

# 两个后端一起编译。SOEM_ROOT 指向 SOEM 2.0.0 的源码目录。
# IgH 需要与目标主站匹配的用户态头文件和 libethercat。
cmake -S native -B build/native -DCMAKE_BUILD_TYPE=Release \
  -DSOEM_ROOT=/path/to/SOEM-v2.0.0 -DMONITOR_IGH=ON
cmake --build build/native --parallel

# SOEM 需要原始套接字权限和设置网卡混杂模式的权限。
sudo setcap cap_net_raw,cap_net_admin+ep "$(realpath build/native/ecat-monitor-soem)"
ECAT_MONITOR_BACKEND_DIR="$(realpath build/native)" ./target/release/ethercat-monitor
```

- 只构建 IgH 时，传入 `-DMONITOR_SOEM=OFF -DMONITOR_IGH=ON`。IgH 默认搜索系统和 `/opt/etherlab`，也可显式设置 `IGH_INCLUDE_DIR`、`IGH_LIBRARY`。确保当前用户有权限以读写方式打开 `/dev/EtherCAT0` 或相应主站设备。
- 只构建 SOEM 时，省略 `-DMONITOR_IGH=ON`。如果 SOEM 2.0.0 已安装且 CMake 能找到其包配置，可以省略 `SOEM_ROOT`。通过 `ip -br link` 查找直连从站的网卡。
- 只运行演示时，只需构建 Rust 程序，选「演示」后点击「连接」。

## Windows 构建与运行

需要 Rust、CMake 3.28+、C 编译器和 Npcap。使用 Npcap 的 WinPcap 兼容安装，并根据 Npcap 的安装选项使用具备抓包权限的账户。以能使用 x64 MSVC 的终端为例：

```powershell
cargo build --release --locked
cmake -S native -B build/native -A x64 "-DSOEM_ROOT=C:/path/to/SOEM-v2.0.0"
cmake --build build/native --config Release
$env:ECAT_MONITOR_BACKEND_DIR = (Resolve-Path build/native/Release).Path
.\target\release\ethercat-monitor.exe
```

可用下列命令获取设备名，将对应的 `\Device\NPF_{...}` 复制到监测器的网卡输入框：

```powershell
Get-NetAdapter | Select-Object Name, @{n='SOEM';e={'\Device\NPF_' + ([guid]$_.InterfaceGuid).ToString('B')}}
```

也可以将后端可执行文件放在 Rust 程序旁边来省略 `ECAT_MONITOR_BACKEND_DIR`。Windows 界面支持 SOEM 和演示。部署 IgH 时在 Linux 上运行程序和 IgH 后端。
