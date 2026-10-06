# The windows `itsanas setup` opens on Windows for the recovery words and the
# passphrase (crates/itsanas-cli/src/setup/secrets.rs).
#
# This text is constant and holds no secret: PowerShell may write a script to
# the event log, and the words must never be in one. They arrive as data on
# standard input, after this script, and the answers leave on standard output
# as hexadecimal UTF-8 -- a redirected console writes in its code page, which
# would change an accented passphrase into another one.
#
# Input lines: mode (show | new-passphrase | passphrase | phrase), then one
# extra line (the positions to ask back, or the purpose in Base64), then the
# words in Base64 for `show`. Exit 0 with the answers, 2 when cancelled.
param([System.IO.TextReader]$In)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$mode = $In.ReadLine()
$extra = $In.ReadLine()
$data = $In.ReadLine()

$font = New-Object System.Drawing.Font('Segoe UI', 12)
$bold = New-Object System.Drawing.Font('Segoe UI', 12, [System.Drawing.FontStyle]::Bold)
$words = New-Object System.Drawing.Font('Consolas', 16, [System.Drawing.FontStyle]::Bold)

function From-Base64([string]$Text) {
    if (-not $Text) { return '' }
    return [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Text))
}

function Write-Answer([string]$Value) {
    $hex = ([Text.Encoding]::UTF8.GetBytes($Value) | ForEach-Object { $_.ToString('x2') }) -join ''
    [Console]::Out.WriteLine($hex)
}

# On top, centred, in the taskbar, and given the focus: a window that opens
# behind the terminal that started it looks like a setup that hung.
function New-Window([string]$Title, [int]$Width, [int]$Height) {
    $form = New-Object System.Windows.Forms.Form
    $form.Text = $Title
    $form.ClientSize = New-Object System.Drawing.Size($Width, $Height)
    $form.StartPosition = 'CenterScreen'
    $form.TopMost = $true
    $form.ShowInTaskbar = $true
    $form.FormBorderStyle = 'FixedDialog'
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.Font = $font
    $form.Add_Shown({ $this.Activate(); $this.BringToFront() })
    return $form
}

function Add-Label($Form, [string]$Text, [int]$X, [int]$Y, [int]$Width, [int]$Height, $Font) {
    $label = New-Object System.Windows.Forms.Label
    $label.Text = $Text
    $label.Location = New-Object System.Drawing.Point($X, $Y)
    $label.Size = New-Object System.Drawing.Size($Width, $Height)
    if ($Font) { $label.Font = $Font }
    $Form.Controls.Add($label)
    return $label
}

function Add-Box($Form, [int]$X, [int]$Y, [int]$Width) {
    $box = New-Object System.Windows.Forms.TextBox
    $box.Location = New-Object System.Drawing.Point($X, $Y)
    $box.Size = New-Object System.Drawing.Size($Width, 30)
    $box.UseSystemPasswordChar = $true
    $Form.Controls.Add($box)
    return $box
}

function Add-Buttons($Form, [string]$Ok, [int]$Y) {
    $width = $Form.ClientSize.Width
    $okButton = New-Object System.Windows.Forms.Button
    $okButton.Text = $Ok
    $okButton.Size = New-Object System.Drawing.Size(240, 38)
    $okButton.Location = New-Object System.Drawing.Point(($width - 380), $Y)
    $okButton.DialogResult = [System.Windows.Forms.DialogResult]::OK
    $cancel = New-Object System.Windows.Forms.Button
    $cancel.Text = 'Cancel'
    $cancel.Size = New-Object System.Drawing.Size(120, 38)
    $cancel.Location = New-Object System.Drawing.Point(($width - 130), $Y)
    $cancel.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
    $Form.Controls.Add($okButton)
    $Form.Controls.Add($cancel)
    $Form.AcceptButton = $okButton
    $Form.CancelButton = $cancel
    return $okButton
}

function Show-Words([string]$Phrase) {
    $list = @($Phrase -split '\s+' | Where-Object { $_ })
    $form = New-Window 'ITSaNAS - your recovery words' 760 520
    [void](Add-Label $form ('Write these 24 words on paper. Anyone with them can read your files. ' +
        'ITSaNAS will never ask for them in a web page.') 20 16 720 56 $bold)
    for ($index = 0; $index -lt $list.Count; $index++) {
        $column = $index % 4
        $row = [Math]::Floor($index / 4)
        $text = '{0,2}. {1}' -f ($index + 1), $list[$index]
        [void](Add-Label $form $text (20 + 182 * $column) (88 + 56 * $row) 178 40 $words)
    }
    [void](Add-Buttons $form 'I have written them down' 460)
    return ($form.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK)
}

function Ask-Back([int[]]$Positions) {
    $form = New-Window 'ITSaNAS - check your recovery words' 560 300
    [void](Add-Label $form 'From your paper, type the words at these positions.' 20 16 520 30 $null)
    $boxes = @()
    for ($index = 0; $index -lt $Positions.Count; $index++) {
        $y = 64 + 52 * $index
        [void](Add-Label $form ('Word number {0}:' -f $Positions[$index]) 20 ($y + 3) 200 30 $null)
        $boxes += Add-Box $form 230 $y 300
    }
    [void](Add-Buttons $form 'Check' 236)
    if ($form.ShowDialog() -ne [System.Windows.Forms.DialogResult]::OK) { return $null }
    return @($boxes | ForEach-Object { $_.Text })
}

function Ask-Secret([string]$Title, [string]$Prompt, [bool]$Twice) {
    $form = New-Window $Title 600 260
    [void](Add-Label $form $Prompt 20 16 560 56 $null)
    $first = Add-Box $form 20 80 560
    $second = $null
    if ($Twice) {
        [void](Add-Label $form 'The same again:' 20 122 300 26 $null)
        $second = Add-Box $form 20 150 560
    }
    $problem = Add-Label $form '' 20 186 560 26 $null
    $problem.ForeColor = [System.Drawing.Color]::Firebrick
    $ok = Add-Buttons $form 'OK' 210
    # Checked here, with the window still open, so a mistyped second copy
    # costs a retype rather than a whole new run of setup.
    $ok.Add_Click({
        if ($first.Text.Length -eq 0) {
            $problem.Text = 'It cannot be empty.'
            $form.DialogResult = [System.Windows.Forms.DialogResult]::None
        } elseif ($Twice -and -not [string]::Equals($first.Text, $second.Text, [StringComparison]::Ordinal)) {
            $problem.Text = 'The two did not match. Type them again.'
            $second.Text = ''
            $form.DialogResult = [System.Windows.Forms.DialogResult]::None
        }
    }.GetNewClosure())
    if ($form.ShowDialog() -ne [System.Windows.Forms.DialogResult]::OK) { return $null }
    return $first.Text
}

switch ($mode) {
    'show' {
        $positions = @($extra -split ' ' | Where-Object { $_ } | ForEach-Object { [int]$_ })
        if (-not (Show-Words (From-Base64 $data))) { exit 2 }
        $typed = Ask-Back $positions
        if ($null -eq $typed) { exit 2 }
        foreach ($word in $typed) { Write-Answer $word }
    }
    'new-passphrase' {
        $value = Ask-Secret 'ITSaNAS - choose a passphrase' ('A passphrase for this machine''s keys. ' +
            'Choose a long one, and write it down: it is asked when this machine starts ITSaNAS.') $true
        if ($null -eq $value) { exit 2 }
        Write-Answer $value
    }
    'passphrase' {
        $value = Ask-Secret 'ITSaNAS - your passphrase' (From-Base64 $extra) $false
        if ($null -eq $value) { exit 2 }
        Write-Answer $value
    }
    'phrase' {
        $value = Ask-Secret 'ITSaNAS - enter your recovery words' ('Your 24 recovery words, in order, ' +
            'separated by spaces. They stay on this machine.') $false
        if ($null -eq $value) { exit 2 }
        Write-Answer $value
    }
    default { exit 3 }
}
exit 0
