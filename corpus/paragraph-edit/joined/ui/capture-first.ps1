Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$app = New-Object -ComObject OneNote.Application
$out = 'C:\one-tests\runs\capture\join-first'
New-Item -ItemType Directory -Path $out | Out-Null
$pages = @('{10598CD2-0723-42D7-BF23-B94ACAD9FE98}{1}{B0}','{488376FA-2C20-49A0-AD63-06E9BB6F864E}{1}{B0}','{5582A13D-6A30-45C6-870C-2C00289B5532}{1}{B0}','{56AD7542-11FB-4FA0-9969-68E0862AB40C}{1}{B0}','{5E5A198A-FAB0-4244-B63D-08CAE25D99F5}{1}{B0}','{5F06FB85-2000-4F62-B60A-C044F19B9B6A}{1}{B0}','{68A9ABE2-3C66-4F1A-B150-67B46DC1D6BE}{1}{B0}','{883508F1-BFB9-451D-B80E-99CE85E616C3}{1}{B0}','{B8B5BDEA-9DE8-4AFA-B38A-FA3F252F7AEF}{1}{B0}','{BEE46007-7505-4670-A7EE-EEB0FA357A71}{1}{B0}','{C8438CC2-32B2-41CC-A696-4BAE6568F396}{1}{B0}','{D2B90D6D-D72E-4720-9D2D-17121D2F2774}{1}{B0}','{F24FC20B-6F88-420C-9835-8735D0222E13}{1}{B0}','{F47A1E7D-C667-4C0A-B942-F9716F98224B}{1}{B0}')
$i = 0
foreach ($id in $pages) {
 $xml = ''
 $app.GetPageContent($id, [ref]$xml, 1, 1)
 [IO.File]::WriteAllText((Join-Path $out ('page-{0:d3}.xml' -f $i)), $xml, [Text.Encoding]::UTF8)
 $i++
}
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
