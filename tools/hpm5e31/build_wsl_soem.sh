set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source_path="${SOEM_ROOT:-/mnt/c/Users/Administrator/Projects/soem-2.0.0/SOEM-v2.0.0}"
build="${HPM_SOEM_BUILD:-${XDG_CACHE_HOME:-$HOME/.cache}/hpm-ethercat/soem-master}"
cmake -S "$root/HostTools/soem_master" -B "$build" -DSOEM_ROOT="$source_path" -DCMAKE_BUILD_TYPE=Release
cmake --build "$build" -j 6
