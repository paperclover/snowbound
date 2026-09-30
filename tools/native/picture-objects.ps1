param([Parameter(Mandatory=$true)][string]$Root, [Parameter(Mandatory=$true)][string]$CloneHost)
# A page of pictures and files, in an outline and on the page, for tagging, linking and OCR
# by hand in the clone: notebook/photo.png, notebook/ocr.png and notebook/notes.txt ride along
# with the section.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\cold-current.ps1" -Root $Root -CloneHost $CloneHost
$namespace = 'http://schemas.microsoft.com/office/onenote/2010/onenote'
$notebook = Join-Path $Root 'notebook'
$data = { param($name) [Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $notebook $name))) }
$file = Join-Path $notebook 'notes.txt'
$app = New-Object -ComObject OneNote.Application
$notebookId = ''
try {
    $app.OpenHierarchy($notebook, '', [ref]$notebookId, 0)
    $sectionId = ''
    $app.OpenHierarchy('files.one', $notebookId, [ref]$sectionId, 0)
    $pageId = ''
    $app.CreateNewPage($sectionId, [ref]$pageId, 0)
    $body = '<one:Title><one:OE><one:T><![CDATA[Pictures and files]]></one:T></one:OE></one:Title>'
    $body += '<one:Outline><one:Position x="36" y="86"/><one:Size width="300" height="200"/><one:OEChildren>'
    $body += '<one:OE><one:T><![CDATA[Picture below]]></one:T></one:OE>'
    $body += '<one:OE><one:Image><one:Size width="120" height="90"/><one:Data>' + (& $data 'photo.png') + '</one:Data></one:Image></one:OE>'
    $body += '<one:OE><one:InsertedFile pathSource="' + $file + '" preferredName="notes.txt"/></one:OE>'
    $body += '<one:OE><one:T><![CDATA[Text after]]></one:T></one:OE>'
    $body += '</one:OEChildren></one:Outline>'
    $body += '<one:Image><one:Position x="400" y="86"/><one:Size width="120" height="90"/><one:Data>' + (& $data 'photo.png') + '</one:Data></one:Image>'
    $body += '<one:InsertedFile pathSource="' + $file + '" preferredName="notes.txt"><one:Position x="560" y="86"/></one:InsertedFile>'
    $body += '<one:Image><one:Position x="400" y="260"/><one:Size width="320" height="80"/><one:Data>' + (& $data 'ocr.png') + '</one:Data></one:Image>'
    $xml = '<one:Page xmlns:one="' + $namespace + '" ID="' + $pageId + '">' + $body + '</one:Page>'
    $app.UpdatePageContent($xml, [DateTime]::MinValue, 1, $true)
    $app.SyncHierarchy($notebookId)
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
