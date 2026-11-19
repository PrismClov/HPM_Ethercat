"""Temporary loopback transport for EtherCAT frames only."""
import os
import select
import socket
import struct
import sys

WINDOWS_PORT = 37685
LINUX_PORT = 37686

def ethercat(frame):
    return len(frame) >= 14 and frame[12:14] == b"\x88\xa4"

def frame_payload(frame):
    end = 16 + (int.from_bytes(frame[14:16], "little") & 0x07ff)
    return frame[:end]

def linux():
    import fcntl
    import subprocess
    with open("/dev/net/tun", "r+b", buffering=0) as tap, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        fcntl.ioctl(tap, 0x400454ca, struct.pack("16sH22x", b"ecat0", 0x1002))
        subprocess.run(["ip", "link", "set", "ecat0", "up"], check=True)
        udp.bind(("127.0.0.1", LINUX_PORT))
        udp.connect(("127.0.0.1", WINDOWS_PORT))
        print("ecat0 ready", flush=True)
        while True:
            ready, _, _ = select.select([tap, udp], [], [])
            if tap in ready:
                frame = os.read(tap.fileno(), 2048)
                if ethercat(frame):
                    udp.send(frame)
            if udp in ready:
                frame = udp.recv(2048)
                if ethercat(frame):
                    os.write(tap.fileno(), frame)

def windows(adapter):
    import collections
    import ctypes as c
    import threading

    os.add_dll_directory(r"C:\Windows\System32\Npcap")
    pcap = c.CDLL(r"C:\Windows\System32\Npcap\wpcap.dll")
    signatures = {
        "pcap_create": ([c.c_char_p, c.c_char_p], c.c_void_p),
        "pcap_activate": ([c.c_void_p], c.c_int),
        "pcap_set_immediate_mode": ([c.c_void_p, c.c_int], c.c_int),
        "pcap_set_snaplen": ([c.c_void_p, c.c_int], c.c_int),
        "pcap_set_promisc": ([c.c_void_p, c.c_int], c.c_int),
        "pcap_set_timeout": ([c.c_void_p, c.c_int], c.c_int),
        "pcap_next_ex": ([c.c_void_p, c.c_void_p, c.c_void_p], c.c_int),
        "pcap_sendpacket": ([c.c_void_p, c.c_char_p, c.c_int], c.c_int),
        "pcap_geterr": ([c.c_void_p], c.c_char_p),
    }
    for name, (args, result) in signatures.items():
        fn = getattr(pcap, name)
        fn.argtypes, fn.restype = args, result

    def open_adapter():
        err = c.create_string_buffer(256)
        handle = pcap.pcap_create(adapter.encode(), err)
        if not handle:
            raise RuntimeError(err.value)
        for fn, val in ((pcap.pcap_set_snaplen, 2048), (pcap.pcap_set_promisc, 1),
                        (pcap.pcap_set_timeout, 1), (pcap.pcap_set_immediate_mode, 1)):
            if fn(handle, val):
                raise RuntimeError(pcap.pcap_geterr(handle))
        if pcap.pcap_activate(handle) < 0:
            raise RuntimeError(pcap.pcap_geterr(handle))
        return handle

    rx, tx = open_adapter(), open_adapter()
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind(("127.0.0.1", WINDOWS_PORT))
    recent = collections.deque(maxlen=64)
    def send():
        while True:
            frame, source = udp.recvfrom(2048)
            if source == ("127.0.0.1", LINUX_PORT) and ethercat(frame):
                recent.append(frame_payload(frame))
                frame = frame.ljust(60, b"\0")
                if pcap.pcap_sendpacket(tx, frame, len(frame)):
                    raise RuntimeError(pcap.pcap_geterr(tx))
    threading.Thread(target=send, daemon=True).start()
    print("Npcap relay ready", flush=True)
    header, data = c.c_void_p(), c.c_void_p()
    while True:
        result = pcap.pcap_next_ex(rx, c.byref(header), c.byref(data))
        if result < 0:
            raise RuntimeError(pcap.pcap_geterr(rx))
        if result == 1:
            size = c.c_uint32.from_address(header.value + 8).value
            frame = c.string_at(data, size)
            if ethercat(frame) and frame_payload(frame) not in recent:
                udp.sendto(frame, ("127.0.0.1", LINUX_PORT))

if __name__ == "__main__":
    if sys.platform == "win32":
        windows(sys.argv[1])
    else:
        linux()
