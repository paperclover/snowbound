"""Run through the old agent's detached spawn endpoint in a disposable clone."""
import os
import ctypes
from ctypes import wintypes
import subprocess
import sys
import time

if len(sys.argv) != 2 or not sys.argv[1].startswith('ONE-M6-') or os.environ.get('COMPUTERNAME') != sys.argv[1]:
    raise SystemExit('The disposable clone hostname does not match.')
parent = os.getppid()
kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
kernel.OpenProcess.restype = wintypes.HANDLE
kernel.TerminateProcess.argtypes = (wintypes.HANDLE, wintypes.UINT)
kernel.TerminateProcess.restype = wintypes.BOOL
kernel.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
kernel.WaitForSingleObject.restype = wintypes.DWORD
kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
kernel.CloseHandle.restype = wintypes.BOOL
handle = kernel.OpenProcess(0x100001, False, parent)
if not handle:
    raise ctypes.WinError(ctypes.get_last_error())
time.sleep(0.5)
try:
    if not kernel.TerminateProcess(handle, 0):
        raise ctypes.WinError(ctypes.get_last_error())
    if kernel.WaitForSingleObject(handle, 5000) != 0:
        raise RuntimeError('The old agent did not terminate.')
finally:
    kernel.CloseHandle(handle)
with open(r'C:\one-tests\agent-restart.log', 'ab') as log:
    subprocess.Popen([sys.executable, r'C:\win7-agent\agent.py'], stdin=subprocess.DEVNULL,
                     stdout=log, stderr=log, close_fds=True,
                     creationflags=subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP)
