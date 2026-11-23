# The box executable isn't on the computer yet. Create its home with the
# same protected, inheritable SID ACL the box checks again at startup.
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
$drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($K))
if ($drive.DriveType -ne 'Fixed' -or $drive.DriveFormat -ne 'NTFS') {
    throw 'Kuvo needs a local NTFS profile.'
}
if (![IO.Directory]::Exists($K)) {
    $security = [Security.AccessControl.DirectorySecurity]::new()
    $security.SetSecurityDescriptorSddlForm("O:$($sid.Value)D:P(A;OICI;FA;;;$($sid.Value))")
    [void][IO.Directory]::CreateDirectory($K, $security)
}
$security = [IO.Directory]::GetAccessControl($K)
$rules = $security.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
$r = if ($rules.Count -eq 1) { $rules[0] } else { $null }
if (!$security.AreAccessRulesProtected -or $security.GetOwner([Security.Principal.SecurityIdentifier]) -ne $sid -or
    !$r -or $r.IdentityReference -ne $sid -or $r.AccessControlType -ne 'Allow' -or
    $r.FileSystemRights -ne 'FullControl' -or $r.InheritanceFlags -ne 3 -or $r.PropagationFlags -ne 0) {
    throw 'Kuvo needs a folder that only your Windows account can read.'
}
$bin = Join-Path $K 'bin'
if ([IO.Directory]::Exists($bin)) {
    $security = [IO.Directory]::GetAccessControl($bin)
    $rules = $security.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
    if ($rules.Count -ne 1 -or $rules[0].IdentityReference -ne $sid -or $rules[0].AccessControlType -ne 'Allow') {
        throw 'Kuvo needs an installation folder that only your Windows account can read.'
    }
}
