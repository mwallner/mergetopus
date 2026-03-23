# Invoke-Build -Task Pack

Task GenerateLicenseInfo {
	Remove-Item ./THIRDPARTY.json -ErrorAction SilentlyContinue
	Exec {
		cargo bundle-licenses --format json --output THIRDPARTY.json
	}
}

Task BuildWin GenerateLicenseInfo, {
	Exec {
		cargo build --target x86_64-pc-windows-gnu --release
	}
}

Task Build GenerateLicenseInfo, {
	Exec {
		cargo build --release
	}
}

Task Pack Build, BuildWin, {
	$isUnix = $IsLinux -or $IsMacOS -or $PSVersionTable.Platform -eq 'Unix'

	$binPath = '.\target\release'
	if ($isUnix) {
		$binPath = '.\target\x86_64-pc-windows-gnu\release'
	}
	Copy-Item "$binPath\*.exe" nuget\tools\.

	Remove-Item .\target\mergetopus.*.nupkg -ErrorAction SilentlyContinue
	Exec {
		$nuspecPath = 'nuget/mergetopus.portable.nuspec'
		if ($isUnix) {
			docker run -t --rm -v "${PWD}:/tmp" -w /tmp chocolatey/choco /bin/bash -c "choco pack $nuspecPath"
		}
		else {
			choco pack $nuspecPath
		}
	}

	Move-Item .\mergetopus.*.nupkg .\target\. -Force
}

Task Clean {
	Remove-Item .\target\release\* -Recurse -ErrorAction SilentlyContinue
	Remove-Item nuget\tools\*.exe -ErrorAction SilentlyContinue
	Remove-Item ./THIRDPARTY.json -ErrorAction SilentlyContinue
	cargo clean
}
