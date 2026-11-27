# Sourced by scripts/check-installers.sh, in its shell and in its order:
# the bootstrap installers, the release signing step and its question.

# ------------------------------------------- the bootstrap installer's checks
#
# install/get.sh is what a tester who cannot compile pastes. Its one defence
# before a binary lands in ~/.local/bin is the size and SHA-256 check against
# the release manifest, and a check nobody has seen refuse anything is a check
# nobody knows works. So it is run here against a fake GitHub: `curl` and
# `uname` are stubs on the PATH, the "release" is a directory, and HOME and the
# prefix are throwaway. A good binary must be installed by rename; a truncated
# one, and one with a byte changed, must be refused with the installed binary
# left as it was.

getbed=$(mktemp -d)
mkdir -p "$getbed/stub" "$getbed/rel" "$getbed/home" "$getbed/prefix/bin"
cat > "$getbed/stub/uname" <<'STUB'
#!/bin/sh
case "$1" in -s) echo Linux ;; -m) echo x86_64 ;; *) echo Linux ;; esac
STUB
# The stub serves the URL's last path segment from the fake release; the API's
# answer is release.json. Arguments are read the way get.sh passes them.
cat > "$getbed/stub/curl" <<'STUB'
#!/bin/sh
out=""; url=""
while [ $# -gt 0 ]; do
    case "$1" in -o) out=$2; shift 2 ;; -*) shift ;; *) url=$1; shift ;; esac
done
case "$url" in */releases/latest) name=release.json ;; *) name=${url##*/} ;; esac
[ -f "$GETREL/$name" ] || exit 22
cp "$GETREL/$name" "$out"
STUB
chmod 755 "$getbed/stub/uname" "$getbed/stub/curl"
printf '{\n  "tag_name": "v9.9.9",\n  "draft": false\n}\n' > "$getbed/rel/release.json"
printf '#!/bin/sh\necho new-release\n' > "$getbed/good"
bin_name=itsanas-x86_64-unknown-linux-gnu
if command -v sha256sum >/dev/null 2>&1; then
    good_sha=$(sha256sum "$getbed/good" | cut -d' ' -f1)
else
    good_sha=$(shasum -a 256 "$getbed/good" | cut -d' ' -f1)
fi
good_size=$(wc -c < "$getbed/good" | tr -d ' ')
zero=0000000000000000000000000000000000000000000000000000000000000000
printf 'itsanas-release 1\nversion 9.9.9\nfile x86_64-unknown-linux-gnu %s %s %s %s\n' \
    "$bin_name" "$good_size" "$zero" "$good_sha" > "$getbed/rel/manifest.txt"

# The prefix's bin is already on the PATH: get.sh then has no PATH question to
# ask. Without it, a developer running this check in a terminal is asked one on
# /dev/tty (stdin redirected or not) and the check waits forever -- found on
# 2026-10-06, when a run under WSL sat on that prompt for twenty minutes.
run_get() {
    PATH="$getbed/stub:$getbed/prefix/bin:$PATH" HOME="$getbed/home" GETREL="$getbed/rel" \
        sh install/get.sh --prefix "$getbed/prefix" --no-setup </dev/null 2>&1
}

printf '#!/bin/sh\necho old-install\n' > "$getbed/prefix/bin/itsanas"
for bad_kind in truncated altered; do
    if [ "$bad_kind" = truncated ]; then
        head -c 5 "$getbed/good" > "$getbed/rel/$bin_name"
    else
        sed 's/new-release/new-relEase/' "$getbed/good" > "$getbed/rel/$bin_name"
    fi
    out=$(run_get)
    if printf '%s' "$out" | grep -q 'error:' \
        && grep -q old-install "$getbed/prefix/bin/itsanas"; then
        say "get.sh refuses a $bad_kind download and leaves the installed binary alone"
    else
        bad "get.sh installed a $bad_kind download, or did not say so:"
        printf '%s\n' "$out" | tail -3 | sed 's/^/       /'
    fi
done

cp "$getbed/good" "$getbed/rel/$bin_name"
out=$(run_get)
if grep -q new-release "$getbed/prefix/bin/itsanas" 2>/dev/null \
    && [ ! -e "$getbed/prefix/bin/.itsanas.new" ]; then
    say "get.sh installs a release whose size and SHA-256 match, by rename"
else
    bad "get.sh did not install a matching release:"
    printf '%s\n' "$out" | tail -3 | sed 's/^/       /'
fi
if [ -n "$(ls -A "$getbed/home")" ]; then
    bad "get.sh wrote into HOME, which it has no reason to touch here: $(ls -A "$getbed/home")"
fi
rm -rf "$getbed"

# The same two refusals for install/get.ps1, the line a Windows tester pastes.
# The network is two functions defined before the script runs (a function wins
# over the cmdlet of the same name). -NoPath always, and only the refusals:
# the success path adds the install directory to the *user's* PATH in the
# registry, and on 2026-10-06 a sabotaged get.ps1 run by this very check did
# exactly that on the developer's machine. -NoPath keeps a broken script away
# from the registry; running only refusals keeps a broken -NoPath away too.
if command -v pwsh >/dev/null 2>&1; then
    psbed=$(mktemp -d)
    mkdir -p "$psbed/rel" "$psbed/prefix/bin"
    printf 'MZ pretend windows binary, new-release\n' > "$psbed/good"
    ps_name=itsanas-x86_64-pc-windows-msvc.exe
    if command -v sha256sum >/dev/null 2>&1; then
        ps_sha=$(sha256sum "$psbed/good" | cut -d' ' -f1)
    else
        ps_sha=$(shasum -a 256 "$psbed/good" | cut -d' ' -f1)
    fi
    printf 'itsanas-release 1\nversion 9.9.9\nfile x86_64-pc-windows-msvc %s %s %s %s\n' \
        "$ps_name" "$(wc -c < "$psbed/good" | tr -d ' ')" "$zero" "$ps_sha" > "$psbed/rel/manifest.txt"
    printf 'old-install\n' > "$psbed/prefix/bin/itsanas.exe"
    for bad_kind in truncated altered; do
        if [ "$bad_kind" = truncated ]; then
            head -c 5 "$psbed/good" > "$psbed/rel/$ps_name"
        else
            sed 's/new-release/new-relEase/' "$psbed/good" > "$psbed/rel/$ps_name"
        fi
        out=$(GETREL="$psbed/rel" GETPREFIX="$psbed/prefix" pwsh -NoProfile -Command '
            $env:PROCESSOR_ARCHITECTURE = "AMD64"
            function Invoke-RestMethod { param($Uri, [switch] $UseBasicParsing) [pscustomobject] @{ tag_name = "v9.9.9" } }
            function Invoke-WebRequest { param($Uri, $OutFile, [switch] $UseBasicParsing)
                Copy-Item -LiteralPath (Join-Path $env:GETREL ($Uri -split "/")[-1]) -Destination $OutFile }
            & ./install/get.ps1 -Prefix $env:GETPREFIX -NoSetup -NoPath
            exit $LASTEXITCODE' 2>&1 </dev/null)
        status=$?
        if [ "$status" -ne 0 ] && printf '%s' "$out" | grep -q 'error:' \
            && grep -q old-install "$psbed/prefix/bin/itsanas.exe" \
            && [ ! -e "$psbed/prefix/bin/itsanas.exe.new" ]; then
            say "get.ps1 refuses a $bad_kind download and leaves the installed binary alone"
        else
            bad "get.ps1 installed a $bad_kind download, or did not say so (exit $status):"
            printf '%s\n' "$out" | tail -3 | sed 's/^/       /'
        fi
    done
    # red_team: the new binary cannot be renamed into place (antivirus lock).
    # The old itsanas.exe must be put back, not left as itsanas.exe.old-*.
    cp "$psbed/good" "$psbed/rel/$ps_name"
    out=$(GETREL="$psbed/rel" GETPREFIX="$psbed/prefix" pwsh -NoProfile -Command '
        $env:PROCESSOR_ARCHITECTURE = "AMD64"
        function Invoke-RestMethod { param($Uri, [switch] $UseBasicParsing) [pscustomobject] @{ tag_name = "v9.9.9" } }
        function Invoke-WebRequest { param($Uri, $OutFile, [switch] $UseBasicParsing)
            Copy-Item -LiteralPath (Join-Path $env:GETREL ($Uri -split "/")[-1]) -Destination $OutFile }
        function Rename-Item { param($LiteralPath, $NewName, $ErrorAction)
            if ($LiteralPath -like "*.new") { throw "locked by antivirus" }
            Microsoft.PowerShell.Management\Rename-Item -LiteralPath $LiteralPath -NewName $NewName }
        & ./install/get.ps1 -Prefix $env:GETPREFIX -NoSetup -NoPath
        exit $LASTEXITCODE' 2>&1 </dev/null)
    status=$?
    if [ "$status" -ne 0 ] && grep -q old-install "$psbed/prefix/bin/itsanas.exe" 2>/dev/null; then
        say "red_team: get.ps1 puts the old itsanas.exe back when the new one cannot be renamed in"
    else
        bad "red_team: get.ps1 left no itsanas.exe (or the wrong one) after a failed rename (exit $status):"
        printf '%s\n' "$out" | tail -3 | sed 's/^/       /'
        ls "$psbed/prefix/bin" | sed 's/^/       /'
    fi
    # red_team: Add-ToUserPath must keep %VARS% and REG_EXPAND_SZ. Run on a
    # throwaway HKCU key, never the person's own Environment key.
    if pwsh -NoProfile -Command 'exit [int](-not $IsWindows)' </dev/null >/dev/null 2>&1; then
        out=$(pwsh -NoProfile -Command '
            $ast = [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path ./install/get.ps1), [ref] $null, [ref] $null)
            $fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq "Add-ToUserPath" }, $true)
            Invoke-Expression $fn.Extent.Text
            $sub = "Software\itsanas-check-" + [guid]::NewGuid().ToString("N")
            $k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($sub)
            $k.SetValue("Path", "%USERPROFILE%\x", [Microsoft.Win32.RegistryValueKind]::ExpandString)
            $k.Close()
            Add-ToUserPath -BinDir "C:\itsanas-check\bin" -KeyPath $sub *> $null
            $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($sub)
            $raw = $k.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $kind = $k.GetValueKind("Path")
            $k.Close()
            [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($sub)
            "$kind|$raw"' 2>&1 </dev/null)
        if [ "$out" = 'ExpandString|%USERPROFILE%\x;C:\itsanas-check\bin' ]; then
            say "red_team: get.ps1 adds to the user PATH keeping %USERPROFILE% and REG_EXPAND_SZ"
        else
            bad "red_team: get.ps1 rewrote the user PATH as: $out"
        fi
    else
        say "not Windows; get.ps1's PATH registry check was not run"
    fi
    rm -rf "$psbed"
else
    say "pwsh is not here; get.ps1's download check was not run"
fi

# ------------------------------------------------ the release signing step
#
# red_team: scripts/sign-release.sh signs a draft anyone with write access to
# the repository can edit. It must check every binary against manifest.txt, and
# the manifest's version against the tag, before the key is touched. Run here
# against a fake `gh` (serving a directory, logging upload and edit) and a fake
# `cargo` that runs the itsanas-release built from this checkout. The good
# draft must reach the "Sign it?" question; the bad ones must stop before it,
# with no upload and no publish.
if command -v cargo >/dev/null 2>&1 && cargo build -q -p itsanas-release 2>/dev/null; then
    signbed=$(mktemp -d)
    tool="$PWD/target/debug/itsanas-release"
    [ -x "$tool" ] || tool="$tool.exe"
    mkdir -p "$signbed/stub" "$signbed/draft"
    printf '#!/bin/sh\nwhile [ "$1" != -- ]; do shift; done; shift\nexec "%s" "$@"\n' "$tool" > "$signbed/stub/cargo"
    cat > "$signbed/stub/gh" <<'STUB'
#!/bin/sh
case "$1 $2" in
    "auth status") exit 0 ;;
    "release download") while [ "$1" != -D ]; do shift; done; cp "$SIGNBED/draft/"* "$2"/ ;;
    "release upload"|"release edit") echo "$*" >> "$SIGNBED/published.log" ;;
    *) exit 1 ;;
esac
STUB
    chmod +x "$signbed/stub/cargo" "$signbed/stub/gh"
    touch "$signbed/key"
    printf 'linux binary\n' > "$signbed/draft/itsanas-x86_64-unknown-linux-gnu"
    "$tool" manifest --version 0.2.0 --dir "$signbed/draft" >/dev/null
    sign_ok=1
    sign_run() {  # tag
        SIGNBED="$signbed" PATH="$signbed/stub:$PATH" ITSANAS_RELEASE_KEY="$signbed/key" \
            sh scripts/sign-release.sh "$1" </dev/null >"$signbed/out" 2>&1
    }
    sign_run v0.2.0
    grep -q 'Sign it and publish it?' "$signbed/out" \
        || { bad "sign-release.sh did not reach the question for an untouched draft:"; tail -3 "$signbed/out" | sed 's/^/    /'; sign_ok=0; }
    for case in flipped tag; do
        rm -f "$signbed/published.log"
        if [ "$case" = flipped ]; then
            printf 'linux binarY\n' > "$signbed/draft/itsanas-x86_64-unknown-linux-gnu"; tag=v0.2.0
        else
            printf 'linux binary\n' > "$signbed/draft/itsanas-x86_64-unknown-linux-gnu"; tag=v0.3.0
        fi
        if sign_run "$tag" || grep -q 'Sign it and publish it?' "$signbed/out" \
            || [ -e "$signbed/published.log" ] || [ -e "$signbed/draft/manifest.txt.sig" ]; then
            bad "red_team: sign-release.sh went on to sign a draft ($case) that does not match its manifest"; sign_ok=0
        fi
    done
    [ "$sign_ok" -eq 1 ] && say "red_team: sign-release.sh refuses a draft with a binary changed or a version not the tag's, before asking to sign"
    rm -rf "$signbed"
else
    say "cargo is not here; sign-release.sh's draft check was not run"
fi

# ------------------------------------------ the signing question, asked well
#
# red_team: "Sign it and publish it?" read a keypress made during release.ps1's
# twenty-minute wait, or after a previous run's `pause`, as an empty answer and
# stopped the release -- twice (hand test, 2026-10-22). Both scripts now ask
# again on an empty line, three times at most, and the PowerShell one first
# drops the keys waiting in the console. The sh confirm() runs here with its
# terminal swapped for a file; the PowerShell Confirm-Yes runs where pwsh is,
# with Read-Host stood in for, and its flush is checked to come first.
confirm_ok=1
confirmbed=$(mktemp -d)
sed -n '/^confirm() {/,/^}/p' scripts/sign-release.sh | sed 's|< /dev/tty|<\&3|' > "$confirmbed/confirm.sh"
grep -q '<&3' "$confirmbed/confirm.sh" \
    || { bad "sign-release.sh's confirm() no longer reads /dev/tty as this check expects"; confirm_ok=0; }
check_sh_confirm() {  # answers (printf format), status wanted, questions wanted
    printf "$1" > "$confirmbed/answers"
    out=$(sh -c '. "$1/confirm.sh"; confirm "Sign it?" "type y to sign and publish, n to stop" 3<"$1/answers"; echo "status=$?"' _ "$confirmbed" 2>&1)
    asked=$(printf '%s\n' "$out" | grep -o 'Sign it? \[y/N\]' | wc -l | tr -d ' ')
    case "$out" in *"status=$2"*) ;; *) asked="status other than $2" ;; esac
    if [ "$asked" != "$3" ]; then
        bad "red_team: sign-release.sh's confirm() given '$1' should end with status $2 after $3 question(s): $asked; it said: $out"
        confirm_ok=0
    fi
}
check_sh_confirm '\n\ny\n' 0 3
check_sh_confirm '\n\n\n' 1 3
check_sh_confirm 'n\n' 1 1
check_sh_confirm '  \nY\n' 0 2
printf '\ny\n' > "$confirmbed/answers"
out=$(sh -c '. "$1/confirm.sh"; confirm "Sign it?" "type y to sign and publish, n to stop" 3<"$1/answers"' _ "$confirmbed" 2>&1)
case "$out" in
    *'type y to sign and publish, n to stop'*) ;;
    *) bad "red_team: sign-release.sh's confirm() asks again on an empty line without saying what to type: $out"; confirm_ok=0 ;;
esac

ps_confirm=$(sed -n '/^function Confirm-Yes {/,/^}/p' scripts/sign-release.ps1 | grep -v '^[[:space:]]*#')
flush_at=$(printf '%s\n' "$ps_confirm" | grep -n 'FlushInputBuffer()' | head -n 1 | cut -d: -f1)
read_at=$(printf '%s\n' "$ps_confirm" | grep -n 'Read-Host' | head -n 1 | cut -d: -f1)
if [ -z "$flush_at" ] || [ -z "$read_at" ] || [ "$flush_at" -ge "$read_at" ]; then
    bad "red_team: sign-release.ps1's Confirm-Yes does not drop the waiting keys before Read-Host"
    confirm_ok=0
fi
grep -q "Confirm-Yes 'Sign it and publish it?' 'type y to sign and publish, n to stop'" scripts/sign-release.ps1 \
    || { bad "sign-release.ps1 asks to sign without saying what to type on an empty answer"; confirm_ok=0; }

if command -v pwsh >/dev/null 2>&1; then
    cat > "$confirmbed/check.ps1" <<'PS'
param([string] $Script)
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Script, [ref]$null, [ref]$null)
$fn = $ast.Find({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Confirm-Yes' }, $true)
if (-not $fn) { 'sign-release.ps1 has no Confirm-Yes'; exit 1 }
. ([scriptblock]::Create($fn.Extent.Text))
$script:answers = @()
$script:asked = 0
$script:said = @()
function Read-Host { param([string] $Prompt)
    $script:asked++
    $next = $script:answers[0]
    $script:answers = @($script:answers | Select-Object -Skip 1)
    return $next
}
function Write-Host { param([string] $Object) $script:said += $Object }
$failures = @()
foreach ($case in @(
        @{ Answers = @('', '', 'y'); Want = $true; Asked = 3; Hints = 2 },
        @{ Answers = @('', '', ''); Want = $false; Asked = 3; Hints = 3 },
        @{ Answers = @('n'); Want = $false; Asked = 1; Hints = 0 },
        @{ Answers = @('  ', 'Oui'); Want = $true; Asked = 2; Hints = 1 })) {
    $script:answers = $case.Answers
    $script:asked = 0
    $script:said = @()
    $got = Confirm-Yes 'Sign it?' 'type y to sign and publish, n to stop'
    $hints = @($script:said | Where-Object { $_ -eq 'type y to sign and publish, n to stop' }).Count
    if ($got -ne $case.Want -or $script:asked -ne $case.Asked -or $hints -ne $case.Hints) {
        $failures += "answers [$($case.Answers -join '|')]: $got after $($script:asked) question(s) and $hints hint(s); wanted $($case.Want) after $($case.Asked) and $($case.Hints)"
    }
}
if ($failures) { $failures; exit 1 }
PS
    if psout=$(pwsh -NoProfile -File "$confirmbed/check.ps1" -Script scripts/sign-release.ps1 2>&1); then
        :
    else
        bad "red_team: sign-release.ps1's Confirm-Yes:"; printf '%s\n' "$psout" | sed 's/^/    /'
        confirm_ok=0
    fi
else
    say "pwsh is not here; sign-release.ps1's Confirm-Yes was checked by reading only"
fi
[ "$confirm_ok" -eq 1 ] && say "red_team: the signing question drops waiting keys (PowerShell) and asks again on an empty line, three times at most"
rm -rf "$confirmbed"
