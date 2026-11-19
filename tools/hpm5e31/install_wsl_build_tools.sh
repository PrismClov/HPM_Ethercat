set -eu
mkdir -p /root/ethercat-apt-lists/partial
cat >/root/ethercat-apt.list <<'EOF'
deb https://archive.ubuntu.com/ubuntu noble main
deb https://archive.ubuntu.com/ubuntu noble-updates main
deb https://security.ubuntu.com/ubuntu noble-security main
EOF
apt_options=(-o Dir::Etc::sourcelist=/root/ethercat-apt.list -o Dir::Etc::sourceparts=- -o Dir::State::lists=/root/ethercat-apt-lists -o Acquire::Languages=none -o Acquire::IndexTargets::deb::DEP-11::DefaultEnabled=false -o Acquire::Retries=1 -o Acquire::https::Timeout=18)
apt-get "${apt_options[@]}" update
DEBIAN_FRONTEND=noninteractive apt-get "${apt_options[@]}" -o DPkg::Lock::Timeout=30 install -y --no-install-recommends gcc libc6-dev make cmake
