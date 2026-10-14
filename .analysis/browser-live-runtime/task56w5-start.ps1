$w='D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256'
$env:ALEPH_HOME="$w/.analysis/browser-live-runtime/home"
$p=Start-Process -FilePath "$w/target/debug/aleph-server.exe" -ArgumentList @('--config',"$w/.analysis/browser-live-runtime/config.toml",'start') -WorkingDirectory $w -WindowStyle Hidden -RedirectStandardOutput "$w/.analysis/browser-live-runtime/server-w5.out" -RedirectStandardError "$w/.analysis/browser-live-runtime/server-w5.err" -PassThru
$p.Id | Out-File -Encoding ascii "$w/.analysis/browser-live-runtime/server-w5.pid"
Start-Sleep 12
Get-NetTCPConnection -LocalPort 18790 -State Listen | ForEach-Object { $q=$_.OwningProcess; $x=Get-CimInstance Win32_Process -Filter "ProcessId=$q"; "$q | $($x.ExecutablePath) | $($x.CommandLine)" }
