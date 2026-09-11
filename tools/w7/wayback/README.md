# wayback — the original physical-box driver

`mcp_win7.py` here is the untouched first version of the Windows 7 MCP server:
it drove a real Windows 7 machine ("wayback") over the network through the same
AutoHotkey listener that the QEMU clones now run, with the box address in `WIN7`
and an optional `WIN7_TOKEN`. Snowbound's lab is VM-only; nothing imports this
file. It stays because the setup was fun and the listener protocol is documented
by it.
