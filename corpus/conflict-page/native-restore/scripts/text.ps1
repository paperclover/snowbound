function Get-ParagraphText([string]$html) {
    $plain = [regex]::Replace($html, '<[^>]*>', '')
    $plain = [regex]::Replace($plain, '&#(?:x([0-9a-fA-F]+)|([0-9]+));', {
        param($match)
        $value = if ($match.Groups[1].Success) {
            [Convert]::ToInt32($match.Groups[1].Value, 16)
        } else { [int]$match.Groups[2].Value }
        # The Win7 framework leaves supplementary numeric entities undecoded.
        if ($value -gt 0xffff -and $value -le 0x10ffff) { [char]::ConvertFromUtf32($value) }
        else { $match.Value }
    })
    [Net.WebUtility]::HtmlDecode($plain)
}
