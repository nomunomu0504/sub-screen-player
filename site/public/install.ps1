# Installs ssp (sub-screen-player) from the latest GitHub release on Windows.
#
#   powershell -c "irm https://subscreen.dev/install.ps1 | iex"
#
# (from PowerShell or the Command Prompt; inside PowerShell, `irm ... | iex` alone works too)
#
# Environment:
#   SSP_VERSION      release to install, e.g. v0.1.0 (default: the latest)
#   SSP_INSTALL_DIR  where to put ssp.exe (default: %LOCALAPPDATA%\Programs\sub-screen-player)

& {
	$ErrorActionPreference = 'Stop'
	$ProgressPreference = 'SilentlyContinue'
	$repo = 'nomunomu0504/sub-screen-player'

	$isArm = $env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64'
	$target = if ($isArm) { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
	$version = $env:SSP_VERSION
	if (-not $version) {
		$version = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
	}
	$dir = $env:SSP_INSTALL_DIR
	if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA 'Programs\sub-screen-player' }

	$name = "ssp-$version-$target"
	$url = "https://github.com/$repo/releases/download/$version"
	$tmp = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
	New-Item -ItemType Directory -Path $tmp | Out-Null
	try {
		Write-Host "Downloading ssp $version for $target"
		Invoke-WebRequest -UseBasicParsing "$url/$name.zip" -OutFile "$tmp\$name.zip"
		Invoke-WebRequest -UseBasicParsing "$url/SHA256SUMS.txt" -OutFile "$tmp\SHA256SUMS.txt"

		$line = Get-Content "$tmp\SHA256SUMS.txt" | Where-Object { $_ -match (' ' + [regex]::Escape("$name.zip") + '$') }
		if (-not $line) { throw "no checksum for $name.zip" }
		$expected = ($line -split '\s+')[0]
		$actual = (Get-FileHash "$tmp\$name.zip" -Algorithm SHA256).Hash.ToLower()
		if ($expected -ne $actual) { throw "checksum mismatch for $name.zip" }

		Expand-Archive "$tmp\$name.zip" -DestinationPath $tmp -Force
		New-Item -ItemType Directory -Force -Path $dir | Out-Null
		Copy-Item "$tmp\$name\ssp.exe" (Join-Path $dir 'ssp.exe') -Force
	}
	finally {
		Remove-Item -Recurse -Force $tmp
	}

	$installed = & (Join-Path $dir 'ssp.exe') --version
	Write-Host "Installed $installed to $dir\ssp.exe"

	$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
	if (-not $userPath) { $userPath = '' }
	if (($userPath -split ';') -notcontains $dir) {
		$newPath = (($userPath.TrimEnd(';'), $dir) | Where-Object { $_ }) -join ';'
		[Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
		$env:Path = "$env:Path;$dir"
		Write-Host "Added $dir to your PATH (new terminals pick it up)."
	}
	Write-Host ''
	Write-Host "Next: plug in the display and run 'ssp serve'."
}
