# w7 — Windows 7 lab VMs on this Mac

Every native check runs OneNote 2010 inside disposable QEMU clones of a sealed
Windows 7 base image. A model drives a clone by writing AutoHotkey v2 scripts:
each call runs on the clone's desktop and comes back with stdout and a
screenshot.

Two halves:

- `payload/` — installed into the base image as `C:\win7-agent`. It starts an
  HTTP listener that executes AHK and screenshots; QEMU forwards it to a host
  loopback port per clone. See [payload/README.txt](payload/README.txt).
- `mcp_win7.py` — stays on the Mac. An MCP server (stdio) for desktop control,
  file transfer, commands, detached processes, and VM lifecycle. System python3,
  stdlib only.

The original physical-box version of the server is kept unchanged in
[wayback/](wayback/README.md); nothing uses it.

## Settings

Copy `.env.example` at the repository root to `.env` and fill in `ONE_VM_HOME`
(the directory holding the base image, `targets.json`, clone overlays and the
Linux appliance) and `ONE_WIN7_ISO` (licensed installation media, used only by
`vm.py install`). Both stay outside the repository. Environment variables of the
same names override the file.

## Wire up an MCP client

Paste [codex-config-snippet.toml](codex-config-snippet.toml) into your client's
MCP configuration with the repository path filled in, then restart it; it should
list `win7` and `one-linux`.

## Smoke test

```sh
./vm.py up alpha --wait
./mcp_win7.py --target alpha health   # listener version info
./mcp_win7.py --target alpha shot     # -> ./screenshot.png
./mcp_win7.py --target alpha exec 'Run("notepad.exe")
WinWait("Untitled - Notepad",, 10)
SendText("hello")'                    # prints stdout/exit, -> ./screenshot.png
./mcp_win7.py --target alpha cmd 'ipconfig'
./vm.py down alpha
```

## Local QEMU VM

`vm.py` runs the Windows 7 build VM on Apple silicon. VM state stays under
`ONE_VM_HOME`; the Windows ISO is opened read-only.

```sh
./vm.py install       # create the disk and boot the Windows installer
./vm.py status win7-build
./vm.py screenshot
./vm.py poweroff      # request a clean Windows shutdown
./vm.py run           # boot the installed system
./vm.py media         # rebuild win7-agent.iso from payload/
./vm.py seal          # validate and publish the stopped build disk
```

Normal boots attach `win7-agent.iso` as a read-only CD. Copy its contents to
`C:\win7-agent`, run `install-autostart.cmd` as administrator, restart, then
verify the forwarded listener:

```sh
./mcp_win7.py --target local health
```

Do not put the build disk or licensed installation media in this repository.
Create the immutable base image only after Windows, Office, and the agent pass
their checks and the VM has shut down cleanly.

## Test VMs

Each clone is a sparse overlay on the read-only base. Its name derives a unique
Windows hostname (`alpha` becomes `ONE-ALPHA`), MAC address, forwarded control
port, authentication token, and MCP target. Two vCPUs and 4 GiB are the defaults.

```sh
./vm.py up alpha --wait           # creates when absent; waits through hostname restart
./vm.py up alpha --display        # visible Cocoa window instead of headless
./mcp_win7.py --target alpha health
./vm.py screenshot alpha
./vm.py status                    # lists every clone
./vm.py down alpha                # clean shutdown, then delete the clone
./vm.py down alpha --preserve-machine  # keep a stopped clone for reproduction
```

The MCP server exposes `win7_vm_up`, `win7_vm_down`, and `win7_vm_status`.
`up` accepts creation settings and a `wait` flag; `down` deletes by default and
accepts `preserve_machine`. Use `win7_spawn` for a long-lived process such as
Procmon so it cannot inherit and hold the command channel.

The control agent starts minimized after login. Its taskbar button remains
available for diagnosis without covering OneNote or changing screenshot-based
test coordinates.

## Windows 10 and 11

`win10` (22H2 x64, emulated like Windows 7, so slow) and `win11` (24H2 arm64,
hardware-virtualized) are further bases for clones. They install unattended and
headless from Microsoft's Media Creation Tool catalog: Pro from the
volume-license image, which installs without a product key and stays
unactivated, with local administrator `one` / `one`. Setup's first logon runs
[unattend/lab-setup.cmd](unattend/lab-setup.cmd), which installs the agent like
the Windows 7 build and restarts into it.

```sh
./windows_media.py win11                 # -> $ONE_VM_HOME/media/win11.iso (brew: wimlib xorriso)
./vm.py install --base win11             # headless; add --display to watch
./mcp_win7.py --target win11-build health  # answers once the build reaches its desktop
./vm.py poweroff win11-build
./vm.py seal --base win11
./vm.py up w11 --base win11 --wait       # clones work as above; --base is kept per clone
./mcp_win7.py --target w11 shot
```

A Windows 11 install reaches its agent in about 10 minutes and a new clone in
one; Windows 10 takes about 55 minutes and 7, so pass `--timeout 900` to its
`up --wait`.

Screens are 1024x768, the largest mode edk2 offers the arm64 guest's framebuffer.
The arm64 guest runs x64 programs, including the agent, through Windows' own
emulation. Windows on Arm has no inbox driver for any NIC QEMU emulates, so
`windows_media.py win11` also keeps Red Hat's signed NetKVM driver from the
virtio-win disc. Transparency is on and DWM's `ForceEffectMode` is set, so Mica
and Acrylic render on the emulated display adapter.

## Abrupt-stop recovery tests

`python3 tools/w7/crash.py windows NAME` and `python3 tools/w7/crash.py linux NAME`
(from the repository root) stop a registered disposable QEMU process without
requesting guest shutdown. They verify its VM/socket identity and preserve its
disk and configuration. Restart with the corresponding `up NAME --wait`, and
finish with `down NAME` to delete the owned machine. The existing MCP lifecycle
continues to perform clean shutdowns.

These stops discard guest memory while host storage remains powered. Pair them
with the library's persisted-image tests when evaluating crash recovery; neither
operation certifies a physical disk's behavior during host power loss.

## Linux Samba VM

`linux_vm.py` creates an SSH-only Debian 13 arm64 appliance from Debian's
official generic-cloud image. The downloaded base is SHA-512 verified and kept
under `ONE_VM_HOME/linux`; instances are sparse copy-on-write
overlays. Cloud-init installs Samba, dnsmasq, CIFS tools, smbclient, and fio.

```sh
./linux_vm.py fetch
./linux_vm.py up samba --wait
./linux_vm.py ssh samba -- 'systemctl is-active smbd dnsmasq'
./linux_vm.py status
./linux_vm.py down samba
```

While it runs, Windows VMs obtain an address on their second NIC and reach the
guest-writable share at `\\192.168.77.1\agent`. The Mac controls and inspects
the appliance over its forwarded SSH port. One appliance runs at a time because
that stable share address is the synchronization point for concurrent Windows
clients. A local VDE switch joins up to 63 guests without host privileges. The
`one-linux` MCP exposes fetch, up, SSH, down, and status; its `up` and `down`
follow the same auto-create, wait-flag, and preserve conventions as Windows.

To restore a base from private object storage, upload the sealed qcow2 beside
its JSON manifest and pass the manifest URL. A presigned URL needs no secret;
otherwise set `ONE_VM_AUTHORIZATION` to the complete HTTP Authorization value.

```sh
./vm.py fetch 'https://private.example/one/win7-office-base.json'
```

`install-autostart.cmd` disables UAC so the isolated test guest can rename
itself, start the control agent, and run Procmon without human prompts. QEMU
publishes each guest agent only on host loopback. Do not run that installer on
a physical or network-reachable Windows machine.

## Gotchas

- `-cpu qemu64` lacks SSSE3, which Mesa's llvmpipe (software OpenGL for Snowbound on a clone)
  needs: `ONE_VM_CPU=Nehalem ./vm.py up NAME --wait` picks another model.
- Keep Windows display scaling at 100%. Anything else and the screenshot
  coordinates no longer match where clicks land.
- The listener must run in the interactive logged-in session — as a scheduled
  task or service it gets session 0 and sees a black screen.
