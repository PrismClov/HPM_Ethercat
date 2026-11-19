set -o pipefail
app=/root/hpm-ethercat/build/ecat_soem_master
base=/mnt/c/Users/Administrator/Documents/ChatGPT/hpm/ecat-ram
python3 "$base/wsl_ethercat_relay.py" &
relay=$!
trap 'kill "$relay" 2>/dev/null || true' EXIT
for attempt in {1..40}; do
  [[ -e /sys/class/net/ecat0 ]] && break
  sleep 0.05
done
python3 "$base/probe_wsl_ethercat.py" || exit 1


  stdbuf -oL -eL "$app" -i ecat0 -c 500 -t 60 --dc --dc-lead-us 150 --cpu 5 --rt-priority 80 --lock-memory 2>&1 |
  tee "$base/wsl_hpm_dc_500us_60s.log"
