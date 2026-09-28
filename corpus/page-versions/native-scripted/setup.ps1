$ErrorActionPreference = 'Stop'
$app = New-Object -ComObject OneNote.Application
$notebook = ''
$app.OpenHierarchy('C:\one-tests\versions\Versions', '', [ref]$notebook, 1)
$section = ''
$app.OpenHierarchy('History.one', $notebook, [ref]$section, 3)
$page = ''
$app.CreateNewPage($section, [ref]$page, 0)
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$app.UpdatePageContent('<one:Page xmlns:one="' + $namespace + '" ID="' + $page + '"><one:Title><one:OE><one:T>Versioned</one:T></one:OE></one:Title><one:Outline><one:Position x="36" y="86"/><one:OEChildren><one:OE><one:T>First state.</one:T></one:OE></one:OEChildren></one:Outline></one:Page>')
$page | Out-File 'C:\one-tests\versions\page.txt' -Encoding UTF8
