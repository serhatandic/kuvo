$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
try {
    $scheduler = New-Object -ComObject 'Schedule.Service'
    $scheduler.Connect()
    $folder = $scheduler.GetFolder('\')
    $name = $env:KUVO_TASK_NAME
    $sid = $env:KUVO_TASK_SID
    $program = $env:KUVO_TASK_PROGRAM
    $task = $null
    try {
        $task = $folder.GetTask($name)
    } catch {
        if ($_.Exception.GetBaseException().HResult -ne -2147024894) { throw }
    }
    if ($null -ne $task) {
        $current = $task.Definition
        $principal = $current.Principal.UserId
        if ($principal -ne $sid) {
            $principal = [Security.Principal.NTAccount]::new($principal).Translate([Security.Principal.SecurityIdentifier]).Value
        }
        if ($principal -ne $sid -or $current.Actions.Count -ne 1 -or
            $current.Actions.Item(1).Path -ne $program -or $current.Actions.Item(1).Arguments) {
            throw 'A different program already uses this task name.'
        }
    }
    switch ($env:KUVO_TASK_ACTION) {
        'register' {
            if ($null -ne $task) { break }
            $definition = $scheduler.NewTask(0)
            $definition.RegistrationInfo.Description = 'Starts Kuvo when you sign in.'
            $definition.Principal.UserId = $sid
            $definition.Principal.LogonType = 3 # TASK_LOGON_INTERACTIVE_TOKEN
            $definition.Principal.RunLevel = 0  # TASK_RUNLEVEL_LUA
            $settings = $definition.Settings
            $settings.Hidden = $true
            $settings.DisallowStartIfOnBatteries = $false
            $settings.StopIfGoingOnBatteries = $false
            $settings.ExecutionTimeLimit = 'PT0S'
            $settings.RestartInterval = 'PT1M'
            $settings.RestartCount = 999
            $settings.MultipleInstances = 2 # TASK_INSTANCES_IGNORE_NEW
            $definition.Triggers.Create(9).UserId = $sid # TASK_TRIGGER_LOGON
            # RestartOnFailure did not retry failed demand starts on Windows 11.
            # The repeating trigger also covers that case. IgnoreNew protects a live run.
            $retry = $definition.Triggers.Create(1) # TASK_TRIGGER_TIME
            $retry.StartBoundary = [DateTime]::UtcNow.AddMinutes(1).ToString('yyyy-MM-ddTHH:mm:ssZ')
            $retry.Repetition.Interval = 'PT1M'
            $action = $definition.Actions.Create(0) # TASK_ACTION_EXEC
            $action.Path = $program
            # Keep control with the installing SID, including when sshd gave it an elevated token.
            $acl = "O:${sid}D:P(A;;GA;;;${sid})(A;;GA;;;SY)"
            [void]$folder.RegisterTaskDefinition($name, $definition, 2, $sid, $null, 3, $acl)
            [Console]::WriteLine('registered')
        }
        'start' {
            if ($null -eq $task) { throw 'The Kuvo sign-in task is missing.' }
            try {
                [void]$task.Run($null)
            } catch {
                switch ($_.Exception.HResult) {
                    -2147216608 { [Console]::WriteLine('sign-in') } # SCHED_E_USER_NOT_LOGGED_ON
                    -2147216609 { } # SCHED_E_ALREADY_RUNNING
                    default { throw }
                }
            }
        }
        'exists' {
            [Console]::WriteLine($(if ($null -ne $task) { 'present' } else { 'absent' }))
        }
        'remove' {
            if ($null -ne $task) {
                try {
                    $folder.DeleteTask($name, 0)
                } catch {
                    # Another removal can win between GetTask and DeleteTask.
                    if ($_.Exception.GetBaseException().HResult -ne -2147024894) { throw }
                }
            }
        }
        default { throw 'Unknown task operation.' }
    }
    # A handled missing-task HRESULT can leave PowerShell's implicit exit status at 1.
    exit 0
} catch {
    [Console]::WriteLine($_.Exception.GetBaseException().Message)
    exit 1
}
