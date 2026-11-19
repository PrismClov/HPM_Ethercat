# EtherCAT 总线监测器

Rust + egui 桌面工作台。通过顶部「连接设置」配置主站，左侧选择从站，中央查看在线 PDO，右侧观察变量，底部查看波形和事件。侧栏和底栏可拖动调整大小，也可通过顶部按钮折叠。工作流参考 [SimpleCAT](https://github.com/NutHero/SimpleCAT-EtherCAT-MasterV1.0.0)。

| 后端 | 平台 | 连接参数 | 行为 |
| --- | --- | --- | --- |
| IgH | Linux | 主站编号，例如 `0` | 打开现有主站，读取内核主站信息和当前从站状态。 |
| SOEM 2.0.0 | Linux / Windows | Linux 网卡名或 Windows Npcap 设备名 | 连接时初始化和扫描总线，随后读 AL 状态寄存器。 |
| 演示 | Linux / Windows | 无 | 模拟三台从站，其中一台周期性显示错误。 |

SOEM 扫描会将从站切到 INIT，并重置部分 ESC 配置和分配站地址。操作前停机，并确保该 EtherCAT 段没有其他主站运行。在状态监测模式下，扫描后保持 INIT。同一段总线更换设备后可断开重连重新识别。IgH 模式可用于观察已经运行的 IgH 主站，AL 状态码列中 `—` 表示接口未提供该字段。参见 [IgH 应用接口](https://docs.etherlab.org/ethercat/1.6/doxygen/group__ApplicationInterface.html) 和 [SOEM 2.0.0 源码](https://github.com/OpenEtherCATsociety/SOEM/tree/v2.0.0)。

## 通用从站调试

切到 SOEM 时会自动枚举本机网卡。选择连接 EtherCAT 的有线网卡，可点「刷新」重新枚举，或点「手动」填写接口 ID。IgH 使用内核中已配置的主站编号。确认独占后点击「扫描从站」。左栏选择从站，点「启动 PDO」后在「过程映像」读取在线变量。实机修改 RxPDO 或操作 CiA402 前，需要在启动会话前勾选「允许本次会话写入输出」。

- 在「连接设置」中选择 FreeRun 或 DC SYNC0。运行中修改周期或模式后点「应用并重连」，工作台释放当前主站，再按新配置启动。重连后输出重新从零开始。
- CiA402 控件随所选从站的在线映射显示。支持一键使能、故障复位、快速停止、禁止电压和模式设置。
- 「ESI 烧录」需在扫描和 PDO 停止后打开，填写 XML 和新的 SII 备份路径。预览会读取在线 SII，生成方案并备份。点击烧录并确认后执行写入和校验。
- `ethercat-monitor --demo` 直接启动模拟 PDO 会话。左栏提供 HPM I/O 和 CiA402 模拟从站。点击「模拟 WKC 归零」，再点击底部事件，可查看该周期的快照。模拟伺服提供「模拟故障」和复位，ESI 页使用内存中的 SII 示例。

`ethercat-service` 是 Rust 调试入口，与桌面程序使用相同的 SOEM 和 IgH 后端可执行文件。先按下文构建原生后端，设置 `ECAT_MONITOR_BACKEND_DIR` 或将后端放在程序旁。扫描、通用 PDO、同步配置和 EEPROM 服务均可从该入口运行。示例中的从站位置从 0 开始。

```bash
cargo build --release --locked --no-default-features --bin ethercat-service
./target/release/ethercat-service scan igh 0 --exclusive
./target/release/ethercat-service process soem enp4s0 1000 freerun --exclusive
./target/release/ethercat-service process igh 0 1000 0x0300 --exclusive --allow-outputs
```

- `process` 列出 PDO 和条目的方向、对象索引、位宽与实际位偏移。`read ID` 读取有效样本，`set ID VALUE` 写入至多 64 位的 RxPDO 原始值。通过 `--allow-outputs` 启用显式写入。过程数据会话请求 OP，初始输出为零。`quit` 关闭会话。
- SOEM 从在线 CoE 映射读取布局，无 CoE 时从 SII 读取。同一从站每个方向支持一个过程数据 SyncManager，布局须与实际 IOmap 一致。IgH 通过内核主站读取在线映射并按条目位置注册 PDO。
- `freerun` 使用主机调度。DC 使用 ESI 中的 SYNC0 AssignActivate，目前支持单从站。SOEM 接受 `0x0300`，并用参考时钟调节发送周期。IgH 用应用时间同步参考时钟及从站时钟。模式在新会话启动时配置。
- CiA402 从映射中匹配控制字 `0x6040:00` 和状态字 `0x6041:00`。`drive 0 status` 显示状态。`drive 0 enable` 按在线状态推进使能序列，`reset` 发送故障复位脉冲，`disable` 禁止电压，`stop` 发送快速停止控制字。映射 `0x6060:00` 时可用 `drive 0 mode 8` 写入模式，映射 `0x6061:00` 时 `status` 显示模式反馈。

### ESI 更新 EEPROM

```bash
./target/release/ethercat-service esi igh 0 0 device.xml original.sii --exclusive
./target/release/ethercat-service esi-plan device.xml original.sii prepared.sii
./target/release/ethercat-service esi soem enp4s0 0 device.xml before-write.sii --exclusive --write
```

默认读取并备份在线 SII，再根据匹配厂商、产品、修订号的 ESI 生成 TxPDO/RxPDO 类别。身份、PDI 配置、CRC、序列号和其他类别从原镜像保留。通过 `--write` 执行写入。备份路径须为新文件。SOEM 逐字写入并读回，IgH 调用与内核主站配套的 `ethercat` 工具写入镜像前缀，随后重扫并核对 SII。可用 `ETHERCAT_TOOL` 指定该工具路径。

Rust 的过程变量、CiA402 和 ESI 流程移植自 SimpleCAT 的 `src/tc3_scan.c`、`ui/ui_master.c` 和 `src/esi_flash.c`，原生后端沿用本仓库的 SOEM 与 IgH 接口。上游版本为 `7bf6be13d05e09aad3045542d7f976ce20d5b444`。 [SimpleCAT 源代码](https://github.com/NutHero/SimpleCAT-EtherCAT-MasterV1.0.0/tree/7bf6be13d05e09aad3045542d7f976ce20d5b444) · [GPL-3.0 许可](COPYING.SimpleCAT)

## HPM5E31-KIT PDO 采集

1. 将单台 HPM5E31-KIT 直连主站网卡，运行仓库中的 P0/PDI 相位诊断固件。程序按厂商 ID `0x00024121`、产品代码 `0x00000001` 以及 CoE PDO 分配和映射校验设备。
2. 选择 IgH 或 SOEM，设置主站编号或网卡。停止该总线上的其他主站应用，确认后扫描并启动 PDO。IgH 模式会独占内核主站并创建 PDO 域，SOEM 模式会初始化总线并请求 SAFE-OP、OP。输出初始值为零。
3. 在左侧选择 HPM5E31-KIT，在中央 `0x6000:00` 行点「+波形」加入原始值，通过「拆分」加入 GPIO 和 PDI 相位。行尾的「位图」展开实时位域。WKC 通道始终显示在时间轴顶部，也可以在观察栏添加 WKC。

| 输入对象 | 字段 | 含义 |
| --- | --- | --- |
| `0x6000:00`，32 位小端 | 位 0–15 | GPIO 输入位图。仓库固件将 IN1 和 IN2 映射到位 0 和位 1。 |
| 同上 | 位 16–31 | PDI 相位，单位 µs。`0xffff` 表示该字段无有效样本。 |

时间轴以本次采集起点为零，记录最多 10 秒，并按过程映像宽度限制内存。各车道共用 X 轴，各自显示 Y 轴范围，波形按像素列保留极值。AL 状态来自主站轮询，周期偏差来自主机采样时间。悬停显示周期和时间，点击红色菱形或事件行按周期号定位。拖动游标或点击「暂停观察」也会冻结右栏快照，实时采集继续进行。点击「恢复实时」返回最新数据。无效 PDO 记录为缺值，`0xffff` 仅使 PDI 相位缺值。退出时保存主站、接口、同步设置、观察列表和面板比例，输出权限在下一次会话中重新确认。

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
