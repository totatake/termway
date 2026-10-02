import os
import subprocess
import socket

# Create socket pair
s1, s2 = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)

# Disable cloexec for s2
os.set_inheritable(s2.fileno(), True)

# Spawn Xwayland
env = os.environ.copy()
env['WAYLAND_SOCKET'] = str(s2.fileno())
env['WAYLAND_DEBUG'] = '1'

print(f"Passing FD {s2.fileno()} to Xwayland")
proc = subprocess.Popen(['Xwayland', ':99'], env=env, pass_fds=[s2.fileno()])

# Wait a bit
try:
    proc.wait(timeout=2)
except subprocess.TimeoutExpired:
    print("Xwayland is running fine!")
    proc.terminate()
