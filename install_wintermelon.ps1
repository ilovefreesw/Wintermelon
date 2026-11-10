#Requires -RunAsAdministrator

# PowerShell script to register Wintermelon with Windows Task Scheduler
# by running the command below in PowerShell terminal launched in admin mode 
# powershell -File "Path to this script"

# Step 1: Enable audit of lock/unlock eventIDs 4800, 4801 using secpol.msc
# Windows search -> Local Security Policy (secpol.msc) 
#                  -> Advanced Audit Policy Configuration 
#                    -> System Audit Policies - Local Group Policy Object 
#                      -> Logon/Logoff 
#                        -> Audit Other Logon/Logoff Events = Success
# Step 2: Set up scheduled tasks using template XML files for lock/unlock event


# Step 1
# auditpol /set /subcategory:"Other Logon/Logoff Events" /success:enable /failure:enable
$auditpol = Join-Path $env:SystemRoot "System32\auditpol.exe"
& $auditpol /set "/subcategory:{0CCE921C-69AE-11D9-BED3-505054503030}" /success:enable /failure:enable
if ($LASTEXITCODE -ne 0) {
    throw "Failed to enable lock/unlock auditing (auditpol exit code $LASTEXITCODE)."
}

# Step 2
$scriptdir = Split-Path -Parent $PSCommandPath
$xml1 = $scriptdir + "\Screen_lock_Save_window_position_Clone_display.xml"
$xml2 = $scriptdir + "\Screen_unlock_Extend_display_Restore_window.xml"
$binary = [string](Join-Path $scriptdir "target\release\wintermelon.exe")
$user = "$env:USERDOMAIN\$env:USERNAME"
$timestamp = Get-Date -format s
if (-not (Test-Path $binary)) {
    throw "Wintermelon executable not found at '$binary'. Build it first with 'cargo build --release'."
}

# Customize XML1 and then register with Task Scheduler
[xml]$schtask1 = Get-Content $xml1
$schtask1.Task.RegistrationInfo.Date = $timestamp.ToString()
$schtask1.Task.RegistrationInfo.Author = $user 
$schtask1.Task.Principals.Principal.UserId = $user
$commandNode1 = $schtask1.GetElementsByTagName("Command").Item(0)
if ($null -eq $commandNode1) {
    throw "Task 1 XML is missing its Command element."
}
$commandNode1.InnerText = $binary
$schtask1.Save($xml1)
Write-Host "Setting up task 1 - Wintermelon screen lock save window position clone display..."
Register-ScheduledTask -Xml (Get-Content $xml1 | Out-String) -TaskName "Wintermelon screen lock save window position clone display" -User $user 

# Customize XML2 and then register with Task Scheduler
[xml]$schtask2 = Get-Content $xml2
$schtask2.Task.RegistrationInfo.Date = $timestamp.ToString()
$schtask2.Task.RegistrationInfo.Author = $user 
$schtask2.Task.Principals.Principal.UserId = $user
$commandNode2 = $schtask2.GetElementsByTagName("Command").Item(0)
if ($null -eq $commandNode2) {
    throw "Task 2 XML is missing its Command element."
}
$commandNode2.InnerText = $binary
$schtask2.Save($xml2)
Write-Host "Setting up task 2 - Wintermelon screen unlock extend display restore window..."
Register-ScheduledTask -Xml (Get-Content $xml2 | Out-String) -TaskName "Wintermelon screen unlock extend display restore window" -User $user 
