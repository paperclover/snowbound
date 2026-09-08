$ErrorActionPreference='Stop'
$app=New-Object -ComObject OneNote.Application
$app.DeletePageContent('{08B3F3DC-1FA5-4903-9EE6-7BD859999367}{1}{B0}', '{F0229B14-633D-47E1-B3F2-7B3AC62468DC}{39}{B0}', [DateTime]::MinValue, $false)
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($app)
