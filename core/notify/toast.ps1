param(
    [string]$Title = "agent-harness",
    [string]$Message = "Tapos na ang prompt!",
    # Silent when another channel (Herdr's configured bell) already plays the sound.
    [switch]$Silent
)
# Windows toast via WinRT; no network, no AI calls. Kept in Notification Center.

[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null

$tEsc = [System.Security.SecurityElement]::Escape($Title)
$mEsc = [System.Security.SecurityElement]::Escape($Message)
$audio = if ($Silent) { '<audio silent="true"/>' } else { '' }

$template = @"
<toast>
  <visual>
    <binding template="ToastGeneric">
      <text>$tEsc</text>
      <text>$mEsc</text>
    </binding>
  </visual>
  $audio
</toast>
"@

$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = [Windows.UI.Notifications.ToastNotification]::new($xml)
$appId = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show($toast)
