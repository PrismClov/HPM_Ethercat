# HPM5E31 联调工具

| 工具 | 用途 |
| --- | --- |
| `test_p0_pdo.py` | Windows Npcap + PySOEM，检查 OP、WKC 和 PDO |
| `verify_pdi_timestamp.py` | 检查 PDI 相位与 GPIO |
| `benchmark_dual_dc.py` | Windows Realtek 双站 SYNC0 扫频，记录漏发时隙、各 WKC 计数、收发耗时和 HPM 的 DC 偏差 |
| `benchmark_p0_timing.py` | 记录 Windows 的发帧周期和 WKC，可用 `--output` 指定 CSV |
| `build_wsl_soem.sh` | 构建仓库中的 Linux SOEM 主站，可设置 `SOEM_ROOT`、`HPM_SOEM_BUILD` |
| `wsl_ethercat_relay.py`、`probe_wsl_ethercat.py`、`run_wsl_hpm_dc.sh` | 历史 WSL 转发和 DC 实验 |
| `install_wsl_build_tools.sh` | 历史 WSL 构建依赖安装脚本 |

历史 WSL 主站默认构建到 `$XDG_CACHE_HOME/hpm-ethercat/soem-master`，无 XDG 缓存设置时使用 `~/.cache/hpm-ethercat/soem-master`。新联调日志默认写入 `build/hpm5e31-lab/`。

Windows 脚本内的 Npcap 设备名来自测试机。换机器时先核对设备名。原生 Linux 直连请使用主站的物理网口。 [Rust 主站和 PDO 回读工具](../../HostTools/rust-monitor/README.md)

双站脚本通过 PySOEM 独占网卡，顺序为 STM32 数字 IO、HPM5E31。用 `python benchmark_dual_dc.py "\Device\NPF_{网卡GUID}" --seconds 300 250 500 1000` 指定每档时长和频率。运行期间所有 PDO 输出为零，结束后关闭 SYNC0、检查两站 SAFE-OP。脚本只调整自身进程优先级。网卡中断调整由实验人员单独设置。

每个周期至多发一个 PDO 帧。`skipped` 统计整段未发送的时隙，`wkc` 对已发帧计数，`late_completions` 单独统计越过下一目标边界的收帧调用。`dc_*` 是 HPM ESC 的 0x092C 偏差，`tail_*` 取最后 60 秒（短测取全段）。预热的 5 秒与测量共用同一发送循环。输出的 JSON 行可保存为实测记录。
