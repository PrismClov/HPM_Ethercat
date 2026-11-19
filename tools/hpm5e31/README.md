# HPM5E31 联调工具

| 工具 | 用途 |
| --- | --- |
| `test_p0_pdo.py` | Windows Npcap + PySOEM，检查 OP、WKC 和 PDO |
| `verify_pdi_timestamp.py` | 检查 PDI 相位与 GPIO |
| `benchmark_p0_timing.py` | 记录 Windows 的发帧周期和 WKC，可用 `--output` 指定 CSV |
| `build_wsl_soem.sh` | 构建仓库中的 Linux SOEM 主站，可设置 `SOEM_ROOT`、`HPM_SOEM_BUILD` |
| `wsl_ethercat_relay.py`、`probe_wsl_ethercat.py`、`run_wsl_hpm_dc.sh` | 历史 WSL 转发和 DC 实验 |
| `install_wsl_build_tools.sh` | 历史 WSL 构建依赖安装脚本 |

历史 WSL 主站默认构建到 `$XDG_CACHE_HOME/hpm-ethercat/soem-master`，无 XDG 缓存设置时使用 `~/.cache/hpm-ethercat/soem-master`。新联调日志默认写入 `build/hpm5e31-lab/`。

Windows 脚本内的 Npcap 设备名来自测试机。换机器时先核对设备名。原生 Linux 直连请使用主站的物理网口。 [Rust 主站和 PDO 回读工具](../../HostTools/rust-monitor/README.md)
