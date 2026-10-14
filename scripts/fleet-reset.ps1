# Wipe every ITSaNAS account on this Windows PC and join the fleet's three
# test accounts again, from the signed release. Run after the VM
# (scripts/fleet-reset.sh vm), which creates the accounts and prints their words:
#
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/SigSegGit/itsanas/main/scripts/fleet-reset.ps1)))
#
# Downloads the three installer scripts it uses, so it needs no checkout.
# Destroys data: every node, every synced folder holding the ITSaNAS marker,
# and old word files in Documents\ITSaNAS and on the Desktop.

$ErrorActionPreference = 'Continue'
$accounts = 'nicolas', 'test', 'mandarine'
$coord = 'itsanas.ngas.fr:9898'
$coordId = '2cfb515fc90749d7248f4af404ecb34b5417e0c1339565c8036a9dde70cf72ce'
$raw = 'https://raw.githubusercontent.com/SigSegGit/itsanas/main/install'
$work = Join-Path ([IO.Path]::GetTempPath()) ("itsanas-reset-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $work | Out-Null
try {
    foreach ($script in 'clean.ps1', 'get.ps1', 'provision.ps1') {
        Invoke-RestMethod "$raw/$script" -OutFile (Join-Path $work $script)
    }

    Write-Host "`n== 1/3 Removing every ITSaNAS account on this PC"
    Get-ChildItem $env:USERPROFILE -Directory -Filter '.itsanas-*' -Force | ForEach-Object {
        & "$work\clean.ps1" -Yes -PurgeAccount -PurgeFolder -DrainFor 0 -Instance $_.Name.Substring(9)
    }
    & "$work\clean.ps1" -Yes -PurgeAccount -PurgeFolder -DrainFor 0
    Get-ChildItem "$env:USERPROFILE\Documents\ITSaNAS", "$env:USERPROFILE\Desktop" -Filter '*mots*.txt' -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Host "  removed $($_.FullName)"; Remove-Item $_.FullName -Force }

    Write-Host "`n== 2/3 Installing the signed release"
    & "$work\get.ps1" -NoSetup
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
    itsanas --version

    Write-Host "`n== 3/3 The accounts: $($accounts -join ', ')"
    $passes = @{}
    foreach ($acct in $accounts) {
        Write-Host "`n---------- account $acct ----------"
        # One passphrase per account, as three people sharing a PC would have.
        do {
            $p1 = Read-Host -AsSecureString "Passphrase for '$acct' on this PC"
            $p2 = Read-Host -AsSecureString 'Again'
            $pass = [Net.NetworkCredential]::new('', $p1).Password
            $same = $pass -and $pass -eq [Net.NetworkCredential]::new('', $p2).Password
            if (-not $same) { Write-Host 'They differ or are empty; again.' }
        } until ($same)
        $passes[$acct] = $pass
        $env:ITSANAS_PASSPHRASE = $pass
        $phrase = Join-Path $work "phrase-$acct.txt"
        Read-Host "Paste the 24 words of '$acct' (one line), then Enter" | Set-Content $phrase
        $instance = @{}
        if ($acct -ne 'nicolas') { $instance.Instance = $acct }
        & "$work\provision.ps1" -NoInstall @instance -PhraseFile $phrase -Username $acct -Pledge 10G -Keep 3G `
            -Folder "$env:USERPROFILE\ITSaNAS-$acct" -Coordinator $coord -CoordinatorDevice $coordId
        Remove-Item $phrase -Force -ErrorAction SilentlyContinue
        if (Test-Path "$env:USERPROFILE\ITSaNAS-$acct") {
            "bonjour de $env:COMPUTERNAME pour $acct, $(Get-Date)" | Set-Content "$env:USERPROFILE\ITSaNAS-$acct\bonjour-$env:COMPUTERNAME.txt"
        }
    }

    Write-Host "`n== Done. One id per account; it must be the same on every machine:"
    foreach ($acct in $accounts) {
        $env:ITSANAS_PASSPHRASE = $passes[$acct]
        $id = if ($acct -eq 'nicolas') { itsanas whoami 2>&1 } else { itsanas --instance $acct whoami 2>&1 }
        Write-Host ("{0,-10} {1}" -f $acct, ($id | Select-Object -Last 1))
    }
} finally {
    Remove-Item Env:ITSANAS_PASSPHRASE -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
