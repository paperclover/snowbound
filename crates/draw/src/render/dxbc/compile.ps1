# Compiles draw.hlsl and translucent.hlsl to the shader model 4.0 bytecode d3d11.rs embeds,
# with the d3dcompiler_47.dll Windows 8.1 and later ship: `powershell -File compile.ps1`.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

[ComImport, Guid("8BA5FB08-5195-40e2-AC58-0D989C3A0102"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface ID3DBlob {
    [PreserveSig] IntPtr GetBufferPointer();
    [PreserveSig] UIntPtr GetBufferSize();
}

public static class Fxc {
    [DllImport("d3dcompiler_47.dll", CharSet = CharSet.Ansi)]
    static extern int D3DCompile(byte[] source, UIntPtr size, string name, IntPtr defines,
        IntPtr include, string entry, string target, uint flags, uint effectFlags,
        out ID3DBlob code, out ID3DBlob errors);

    static byte[] Bytes(ID3DBlob blob) {
        var bytes = new byte[(int)blob.GetBufferSize()];
        Marshal.Copy(blob.GetBufferPointer(), bytes, 0, bytes.Length);
        return bytes;
    }

    public static byte[] Compile(string path, string entry, string target) {
        var source = File.ReadAllBytes(path);
        ID3DBlob code, errors;
        // D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3
        int result = D3DCompile(source, (UIntPtr)source.Length, Path.GetFileName(path),
            IntPtr.Zero, IntPtr.Zero, entry, target, (1u << 11) | (1u << 15), 0,
            out code, out errors);
        if (result < 0) {
            throw new Exception(errors == null ? "D3DCompile: 0x" + result.ToString("x8")
                : Encoding.ASCII.GetString(Bytes(errors)));
        }
        return Bytes(code);
    }
}
'@
foreach ($shader in @(
    @('..\..\draw.hlsl', 'draw'),
    @('..\translucent.hlsl', 'translucent')
)) {
    $source = Join-Path $PSScriptRoot $shader[0]
    foreach ($stage in @(@('vertex', 'vs_4_0'), @('fragment', 'ps_4_0'))) {
        $out = Join-Path $PSScriptRoot ($shader[1] + '.' + $stage[0] + '.dxbc')
        [IO.File]::WriteAllBytes($out, [Fxc]::Compile($source, $stage[0], $stage[1]))
    }
}
