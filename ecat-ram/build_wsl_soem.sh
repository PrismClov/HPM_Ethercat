set -euo pipefail
base=/mnt/c/Users/Administrator/Documents/ChatGPT/hpm/ecat-ram
source_path=/mnt/c/Users/Administrator/Projects/soem-2.0.0/SOEM-v2.0.0
build=/root/hpm-ethercat/build
cmake -S "$base/orin-master/HostTools/soem_master" -B "$build" -DSOEM_ROOT="$source_path" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build" -j 6
