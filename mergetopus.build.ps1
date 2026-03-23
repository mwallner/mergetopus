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
	$binPath = '.\target\release'
	if ($PSVersionTable.Platform -ne 'Windows') {
		$binPath = '.\target\x86_64-pc-windows-gnu\release'
	}
	Copy-Item "$binPath\*.exe" nuget\tools\.

	Remove-Item .\target\mergetopus.*.nupkg -ErrorAction SilentlyContinue
	Exec {
		if ($PSVersionTable.Platform -ne 'Windows') {
			docker run -t --rm -v "${PWD}:/tmp" -w /tmp chocolatey/choco /bin/bash -c 'choco pack nuget/mergetopus.nuspec'
		}
		else {
			choco pack nuget/mergetopus.portable.nuspec
		}
	}

	Remove-Item .\target\*.nupkg -ErrorAction SilentlyContinue
	Move-Item .\mergetopus.*.nupkg .\target\.
}

Task Clean {
	Remove-Item .\target\release\* -Recurse -ErrorAction SilentlyContinue
	Remove-Item nuget\tools\*.exe -ErrorAction SilentlyContinue
	Remove-Item ./THIRDPARTY.json -ErrorAction SilentlyContinue
	cargo clean
}
