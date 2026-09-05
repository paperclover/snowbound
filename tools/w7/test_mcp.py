import unittest

import mcp_linux
import mcp_win7


class McpSurfaceTest(unittest.TestCase):
    def test_windows_lifecycle_is_up_down_status(self):
        tools = {tool["name"]: tool for tool in mcp_win7.TOOLS}
        lifecycle = {name for name in tools if name.startswith("win7_vm_")}
        self.assertEqual(lifecycle, {
            "win7_vm_up", "win7_vm_down", "win7_vm_status",
        })
        self.assertIn("wait", tools["win7_vm_up"]["inputSchema"]["properties"])
        self.assertIn(
            "preserve_machine",
            tools["win7_vm_down"]["inputSchema"]["properties"],
        )

    def test_linux_lifecycle_is_fetch_up_down_status(self):
        tools = {tool["name"]: tool for tool in mcp_linux.TOOLS}
        lifecycle = {name for name in tools if name.startswith("linux_vm_")}
        self.assertEqual(lifecycle, {
            "linux_vm_fetch", "linux_vm_up", "linux_vm_down", "linux_vm_status",
        })
        self.assertIn("wait", tools["linux_vm_up"]["inputSchema"]["properties"])
        self.assertIn(
            "preserve_machine",
            tools["linux_vm_down"]["inputSchema"]["properties"],
        )


if __name__ == "__main__":
    unittest.main()
