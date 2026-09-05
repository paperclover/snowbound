win7-agent -- remote computer-use listener for Windows 7 x64
============================================================

WHAT IT DOES
  Listens on port 8777 and executes AutoHotkey scripts and shell commands sent
  by the controller, returning their output plus a screenshot of this desktop.

SETUP
  1. Copy this whole folder anywhere on the machine, e.g. C:\win7-agent
     (keep the vendor\ subfolder next to agent.py -- do not move pieces apart).
  2. Double-click run.bat. A console window opens and prints "listening ...".
  3. When Windows Firewall pops up, click "Allow access" so the controller can
     reach port 8777.

VM AUTOSTART
  On the isolated QEMU build VM only, run install-autostart.cmd as administrator
  and restart. It disables UAC, reads the clone hostname and token from its
  instance CD, renames the guest on first boot, and starts this agent at login.
  Do not use it on a physical or network-reachable Windows machine.

IMPORTANT
  - Must run in the interactive, logged-in desktop session (just double-click
    it). Do NOT install it as a Windows service -- a service has no desktop and
    AutoHotkey cannot move the mouse or type.
  - The agent holds the display and the machine awake by itself while it runs,
    so you do not need to change the power plan. Two things it CANNOT hold off,
    which will silently break every screenshot and keystroke until someone logs
    back in -- turn both off once:
      * Screen saver: right-click desktop > Personalize > Screen Saver, set
        "(None)", and clear "On resume, display logon screen".
      * Lock: do not press Win+L, and if the machine is on a domain policy that
        locks it, that policy has to go.
    A locked session moves the desktop to Winlogon, where nothing this agent
    does can reach. The monitor merely powering off is fine -- screenshots
    still work with the screen dark.
  - Set display scaling to 100%: Control Panel > Display > "Smaller - 100%",
    then log out and back in. Any other scaling shifts screenshot pixels away
    from click coordinates and every click lands in the wrong place.
  - If python.exe fails complaining about api-ms-win-crt-runtime-l1-1-0.dll,
    run vendor\vc_redist.x64.exe once, then start run.bat again.
  - Leave the console window open; closing it stops the agent. A crash leaves
    the window up (pause) so you can read the error.
  - Everything is also written to agent.log beside agent.py: one timestamped
    line per request, followed by the exact AutoHotkey script or cmd command
    that ran, indented with "|". It appends forever and is never rotated, so
    delete it yourself if it gets large.

SHARE (needed for the OneNote work)
  Map the NAS share to A: once, from a normal cmd window:
      net use A: \\zenith\agent /user:agent agent /persistent:yes
  If the hostname does not resolve, use \\10.0.0.1\agent instead. Check it
  stuck with:  net use

TOKEN (optional)
  To require a shared secret, put it in a file named token.txt beside run.bat
  (single line). The controller must then send the same value. No file means
  no authentication.
