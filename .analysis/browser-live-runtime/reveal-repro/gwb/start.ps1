# Isolated cold Gateway B for the Task #56 reveal repro (pre-fix exe, fresh ALEPH_HOME, port 18792).
$w='D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256'
$g="$w/.analysis/browser-live-runtime/reveal-repro/gwb"
$env:ALEPH_HOME="$g/home"
$p=Start-Process -FilePath "$w/target/debug/aleph-server.exe" -ArgumentList @('--config',"$g/config.toml",'start') -WorkingDirectory $w -WindowStyle Hidden -RedirectStandardOutput "$g/server.out" -RedirectStandardError "$g/server.err" -PassThru
$p.Id | Out-File -Encoding ascii "$g/server.pid"
Start-Sleep 15
Get-NetTCPConnection -LocalPort 18792 -State Listen -ErrorAction SilentlyContinue | ForEach-Object { "listen pid=$($_.OwningProcess)" }
