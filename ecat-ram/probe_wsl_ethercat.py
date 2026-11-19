import socket
import struct
import time

iface = 'ecat0'
mac = bytes.fromhex(open('/sys/class/net/' + iface + '/address').read().strip().replace(':', ''))
frame = (b'\xff' * 6 + mac + b'\x88\xa4' + struct.pack('<HBBHHHHHH', 0x100e, 7, 0x31, 0, 0, 2, 0, 0, 0)).ljust(60, b'\0')
with socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x88a4)) as sock:
    sock.bind((iface, 0))
    sock.settimeout(0.15)
    for attempt in range(3):
        sock.send(frame)
        until = time.monotonic() + .15
        while time.monotonic() < until:
            try:
                packet, address = sock.recvfrom(1518)
            except TimeoutError:
                break
            if address[2] != socket.PACKET_OUTGOING and len(packet) >= 30 and packet[16:18] == b'\x07\x31':
                wkc = int.from_bytes(packet[28:30], 'little')
                print('EtherCAT probe reply WKC=', wkc, 'ESC type=', packet[26])
                if wkc:
                    raise SystemExit(0)
    print('No EtherCAT reply in WSL on', iface)
    raise SystemExit(1)
