# Privacy Policy for Tessera

Last updated: October 19, 2026

This policy describes how Tessera handles monitoring data and settings.

## Local monitoring

Tessera reads Windows network-interface byte counters to calculate upload
and download speeds. It reads local CPU, GPU, and physical memory counters, and
available CPU temperature sensors. It does not inspect network packet contents,
probe external servers, or read Wi-Fi network names or location information.

CPU temperature may be read from a compatible, separately running local
LibreHardwareMonitor or OpenHardwareMonitor WMI provider. Tessera does not
install or launch those programs.

Measurements and session totals are held in memory while the application runs.
The application does not write a monitoring history database.

## Local settings

Settings are saved in
`%APPDATA%\io.tessera\settings.json`. They include the
selected adapter and taskbar position. An adapter name may contain a name chosen
by the user. Settings are not transmitted. Uninstalling preserves this file.

## External services

The application includes no analytics, advertising, telemetry, automatic update
checks, or automatic crash reporting. It does not send measurements or settings
to an external service.

If you choose to report an issue on GitHub, review any text or screenshots for
personal information before sharing them. GitHub handles information you submit
there under its own privacy policy.
