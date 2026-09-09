"""Exercise the production executable through a real pseudo-terminal."""
import errno
import fcntl
import os
import pty
import re
import select
import struct
import sys
import termios
import time

binary, source = sys.argv[1:]
pid, master = pty.fork()
if pid == 0:
    os.execv(binary, [binary, source, "--terminal"])

def resize(width, height):
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))

output = b""
def wait_for(text, absent=None):
    global output
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0.1)[0]:
            try:
                data = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not data:
                break
            output += data
        frame = output.split(b"\x1b[2J")[-1]
        clean = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", frame)
        if text in clean and (absent is None or absent not in clean):
            return clean
    raise AssertionError((text, output[-3000:]))

try:
    resize(120, 40)
    screen = wait_for(b"Schema objects")
    assert b"PRIVATE_ROW" not in screen
    os.write(master, b"g2:0\r")
    wait_for(b"Cell 0")
    os.write(master, b"d")
    wait_for(b"PRIVATE_ROW")
    os.write(master, b"[")
    screen = wait_for(b"Revision 1")
    assert b"PRIVATE_ROW" not in screen
    resize(30, 10)
    wait_for(b"Narrow terminal")
    os.write(master, b"\x1b")
    wait_for(b"Database / Page 2", absent=b"/ Cell 0")
    os.write(master, b"\r")
    wait_for(b"Database / Page 2 / Cell 0")
    os.write(master, b"\x1b[6~")
    wait_for(b"Page 2 |")
    os.write(master, b"\x1b[6~")
    wait_for(b"Cells declared: 1")
    resize(120, 40)
    wait_for(b"Main-file image")
    os.write(master, b"q")
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        child, status = os.waitpid(pid, os.WNOHANG)
        if child:
            assert os.waitstatus_to_exitcode(status) == 0
            pid = None
            break
        time.sleep(0.02)
    assert pid is None, "terminal process did not exit"
    # Restored termios is observable through the PTY even after the child exits.
    attributes = termios.tcgetattr(master)
    assert attributes[3] & termios.ICANON
    assert attributes[3] & termios.ECHO
    print("PTY navigation, deep disclosure, revisions, resize, quit and terminal restoration passed")
finally:
    if pid is not None:
        os.kill(pid, 9)
        os.waitpid(pid, 0)
    os.close(master)
