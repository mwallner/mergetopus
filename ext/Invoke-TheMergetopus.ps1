#requires -Version 7.4

<#
	.SYNOPSIS
	Splits a difficult merge into one integration branch plus per-conflict slice branches.

	.DESCRIPTION
	Strategy:

	1) Remember current HEAD.
	2) Create: _mmm/<currentbranch>/<MergeSource>/integration
	3) Attempt merge of <MergeSource> into integration branch (--no-commit).
	4) Keep only non-conflicting auto-merged files in the integration merge commit.
	   Conflicted paths are reset to "ours" (remembered HEAD) for this commit.
	5) For each conflicted file:
	   - Create: _mmm/<currentbranch>/<MergeSource>/slice<num> from merge base (HEAD vs MergeSource).
	   - Apply only that single file from <MergeSource> ("theirs" for that file).
	   - Create a normal single-parent commit on that slice branch.
	   - Preserve provenance in commit trailers.
	   - Set the slice commit author to the most recent source-side author for that path.
		 The user running the cmdlet remains the committer.
	6) Team can parallelize resolution by merging slice branches back into integration branch.

	Notes on authorship:

	- Non-conflicting changes are preserved by the normal merge commit on the integration branch.
	- Slice commits cannot retain original ancestry without changing merge topology.
	- To preserve attribution, each slice commit:
		* uses the source-side path author as Git author
		* keeps the merger as Git committer
		* adds Co-authored-by and provenance trailers

	ASCII overview:

		(start) current branch @ H
				 |
				 +--> integration branch: _mmm/main/feature_x/integration
						|
						+-- merge feature/x (partial)
						|      - non-conflict files kept
						|      - conflict files reset to ours
						|
						+-- conflict list: A.cs, B.cs, C.cs
							   |         |         |
							   |         |         +--> slice3 (from merge-base): only C.cs from source
							   |         +------------> slice2 (from merge-base): only B.cs from source
							   +----------------------> slice1 (from merge-base): only A.cs from source

		Later:
			merge slice1, slice2, slice3 -> integration branch

	.PARAMETER MergeSource
	Git ref to merge from (branch, tag, commit, or other valid ref).

	.EXAMPLE
	Invoke-TheMergetopus -MergeSource feature/refactor-auth

	Creates:
	  - _mmm/<current>/feature_refactor-auth/integration
	  - _mmm/<current>/feature_refactor-auth/slice1..N
	#>
[CmdletBinding(SupportsShouldProcess)]
param(
	[Parameter(Mandatory, Position = 0)]
	[ValidateNotNullOrEmpty()]
	[string]$MergeSource
)

function Invoke-Git {
	param(
		[Parameter(Mandatory)]
		[string[]]$Arguments,
		[switch]$AllowFailure
	)

	$output = & git @Arguments 2>&1
	$exit = $LASTEXITCODE

	if (-not $AllowFailure -and $exit -ne 0) {
		$argText = ($Arguments -join ' ')
		$msg = @(
			"git $argText failed (exit $exit)."
			($output | ForEach-Object { $_.ToString() })
		) -join [Environment]::NewLine
		throw $msg
	}

	[pscustomobject]@{
		ExitCode = $exit
		Output   = @($output | ForEach-Object { $_.ToString() })
	}
}

function ConvertTo-SafeBranchFragment {
	param([Parameter(Mandatory)][string]$Text)
	return ($Text -replace '[^0-9A-Za-z._-]+', '_').Trim('_')
}

function Test-MergeInProgress {
	$r = Invoke-Git -Arguments @('rev-parse', '-q', '--verify', 'MERGE_HEAD') -AllowFailure
	return $r.ExitCode -eq 0
}

function Test-PathExistsInRef {
	param(
		[Parameter(Mandatory)][string]$Ref,
		[Parameter(Mandatory)][string]$Path
	)
	$r = Invoke-Git -Arguments @('cat-file', '-e', "$Ref`:$Path") -AllowFailure
	return $r.ExitCode -eq 0
}

function Get-PathProvenance {
	param(
		[Parameter(Mandatory)][string]$SourceRef,
		[Parameter(Mandatory)][string]$SourceSha,
		[Parameter(Mandatory)][string]$Path
	)

	$pathCommit = $null
	$authorName = $null
	$authorEmail = $null
	$authorDate = $null

	# Most recent commit on the source side that touched this path.
	$format = '%H%x1f%an%x1f%ae%x1f%aI'
	$logResult = Invoke-Git -Arguments @('log', '-n', '1', "--format=$format", $SourceSha, '--', $Path) -AllowFailure

	if ($logResult.ExitCode -eq 0 -and $logResult.Output.Count -gt 0 -and -not [string]::IsNullOrWhiteSpace($logResult.Output[0])) {
		$parts = $logResult.Output[0] -split [char]0x1f
		if ($parts.Count -ge 4) {
			$pathCommit = $parts[0]
			$authorName = $parts[1]
			$authorEmail = $parts[2]
			$authorDate = $parts[3]
		}
	}

	[pscustomobject]@{
		SourceRef    = $SourceRef
		SourceCommit = $SourceSha
		Path         = $Path
		PathCommit   = $pathCommit
		AuthorName   = $authorName
		AuthorEmail  = $authorEmail
		AuthorDate   = $authorDate
	}
}

function Set-TemporaryGitAuthor {
	param(
		[string]$Name,
		[string]$Email,
		[string]$Date
	)

	$state = [pscustomobject]@{
		GIT_AUTHOR_NAME  = $env:GIT_AUTHOR_NAME
		GIT_AUTHOR_EMAIL = $env:GIT_AUTHOR_EMAIL
		GIT_AUTHOR_DATE  = $env:GIT_AUTHOR_DATE
	}

	if ($Name) {
		$env:GIT_AUTHOR_NAME = $Name
	}
	else {
		Remove-Item Env:GIT_AUTHOR_NAME -ErrorAction SilentlyContinue
	}

	if ($Email) {
		$env:GIT_AUTHOR_EMAIL = $Email
	}
	else {
		Remove-Item Env:GIT_AUTHOR_EMAIL -ErrorAction SilentlyContinue
	}

	if ($Date) {
		$env:GIT_AUTHOR_DATE = $Date
	}
	else {
		Remove-Item Env:GIT_AUTHOR_DATE -ErrorAction SilentlyContinue
	}

	return $state
}

function Restore-TemporaryGitAuthor {
	param(
		[Parameter(Mandatory)]
		[pscustomobject]$State
	)

	if ($null -ne $State.GIT_AUTHOR_NAME) {
		$env:GIT_AUTHOR_NAME = $State.GIT_AUTHOR_NAME
	}
	else {
		Remove-Item Env:GIT_AUTHOR_NAME -ErrorAction SilentlyContinue
	}

	if ($null -ne $State.GIT_AUTHOR_EMAIL) {
		$env:GIT_AUTHOR_EMAIL = $State.GIT_AUTHOR_EMAIL
	}
	else {
		Remove-Item Env:GIT_AUTHOR_EMAIL -ErrorAction SilentlyContinue
	}

	if ($null -ne $State.GIT_AUTHOR_DATE) {
		$env:GIT_AUTHOR_DATE = $State.GIT_AUTHOR_DATE
	}
	else {
		Remove-Item Env:GIT_AUTHOR_DATE -ErrorAction SilentlyContinue
	}
}

# Validate repository context.
$insideRepo = Invoke-Git -Arguments @('rev-parse', '--is-inside-work-tree')
if (($insideRepo.Output | Select-Object -First 1) -ne 'true') {
	throw 'Current directory is not inside a Git working tree.'
}

# Require clean working tree to avoid accidental data loss while switching branches.
$dirty = Invoke-Git -Arguments @('status', '--porcelain')
if ($dirty.Output.Count -gt 0) {
	throw 'Working tree is not clean. Commit/stash changes before running Invoke-TheMergetopus.'
}

# Resolve current branch / HEAD.
$currentBranchResult = Invoke-Git -Arguments @('symbolic-ref', '--quiet', '--short', 'HEAD') -AllowFailure
$headSha = (Invoke-Git -Arguments @('rev-parse', '--verify', 'HEAD')).Output[0]

$currentBranch = if ($currentBranchResult.ExitCode -eq 0 -and $currentBranchResult.Output.Count -gt 0) {
	$currentBranchResult.Output[0]
}
else {
	"detached_$($headSha.Substring(0, 8))"
}

# Validate merge source resolves to a commit.
$sourceVerify = Invoke-Git -Arguments @('rev-parse', '--verify', "$MergeSource`^{commit}") -AllowFailure
if ($sourceVerify.ExitCode -ne 0) {
	throw "MergeSource '$MergeSource' is not a valid commit-ish ref."
}
$sourceSha = $sourceVerify.Output[0]
$mergeBaseSha = (Invoke-Git -Arguments @('merge-base', $headSha, $sourceSha)).Output[0]

$safeSource = ConvertTo-SafeBranchFragment -Text $MergeSource
if ([string]::IsNullOrWhiteSpace($safeSource)) {
	$safeSource = 'source'
}

$safeCurrent = ConvertTo-SafeBranchFragment -Text $currentBranch
if ([string]::IsNullOrWhiteSpace($safeCurrent)) {
	$safeCurrent = 'current'
}

$branchPrefix = "_mmm/$safeCurrent/$safeSource"

$integrationBranch = "$branchPrefix/integration"
$sliceBranches = [System.Collections.Generic.List[string]]::new()

if ($PSCmdlet.ShouldProcess($integrationBranch, "Create/reset integration branch at $headSha")) {
	Invoke-Git -Arguments @('checkout', '-B', $integrationBranch, $headSha) | Out-Null
}

# Attempt merge without committing so we can split conflicts.
$mergeAttempt = Invoke-Git -Arguments @('merge', '--no-ff', '--no-commit', $MergeSource) -AllowFailure
if ($mergeAttempt.ExitCode -ne 0) {
	Write-Verbose 'Merge returned non-zero (likely conflicts). Continuing with conflict slicing.'
}

# Collect conflicted paths.
$conflictedFiles = @(
	(Invoke-Git -Arguments @('diff', '--name-only', '--diff-filter=U')).Output |
	Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
)

# Precompute slice mapping so commit message can include target branch names.
$slicePlan = @()
for ($planIndex = 0; $planIndex -lt $conflictedFiles.Count; $planIndex++) {
	$file = $conflictedFiles[$planIndex]
	$branch = "$branchPrefix/slice$($planIndex + 1)"
	$slicePlan += [pscustomobject]@{
		File   = $file
		Branch = $branch
	}
}

# Keep integration merge commit conflict-free by restoring ours for conflicted files.
foreach ($path in $conflictedFiles) {
	Invoke-Git -Arguments @('restore', '--source=HEAD', '--staged', '--worktree', '--', $path) | Out-Null
}

# Gather auto-merged files that remain staged after conflicted paths were restored.
$autoMergedFiles = @(
	(Invoke-Git -Arguments @('diff', '--cached', '--name-only')).Output |
	Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
)

# Finalize integration merge commit if merge is in progress.
if (Test-MergeInProgress) {
	$mergedSection = if ($autoMergedFiles.Count -gt 0) {
		(($autoMergedFiles | ForEach-Object { "* $_" }) -join "`n")
	}
	else {
		'* (none)'
	}

	$slicedSection = if ($slicePlan.Count -gt 0) {
		(($slicePlan | ForEach-Object { "* $($_.File) -> $($_.Branch)" }) -join "`n")
	}
	else {
		'* (none)'
	}

	$msg = @"
TheMergetopus: partial merge '$MergeSource' into '$integrationBranch' (conflicts sliced)

merged:
$mergedSection

sliced:
$slicedSection
"@
	Invoke-Git -Arguments @('commit', '--allow-empty', '-m', $msg) | Out-Null
}

# Build one slice branch per conflicted file from merge base.
$i = 1
foreach ($path in $conflictedFiles) {
	$sliceNumber = $i
	$sliceBranch = "$branchPrefix/slice$sliceNumber"
	$i++

	if ($PSCmdlet.ShouldProcess($sliceBranch, "Create/reset slice branch at $mergeBaseSha")) {
		Invoke-Git -Arguments @('checkout', '-B', $sliceBranch, $mergeBaseSha) | Out-Null
	}

	# "theirs for this one file": take file state from MergeSource.
	if (Test-PathExistsInRef -Ref $MergeSource -Path $path) {
		Invoke-Git -Arguments @('restore', "--source=$MergeSource", '--staged', '--worktree', '--', $path) | Out-Null
	}
	else {
		# File does not exist in source ref, so "theirs" means deletion.
		Invoke-Git -Arguments @('rm', '--ignore-unmatch', '--', $path) | Out-Null
	}

	# Commit only if staged content changed.
	$stagedCheck = Invoke-Git -Arguments @('diff', '--cached', '--quiet') -AllowFailure
	if ($stagedCheck.ExitCode -ne 0) {
		$provenance = Get-PathProvenance -SourceRef $MergeSource -SourceSha $sourceSha -Path $path

		$trailers = @(
			"Source-Ref: $($provenance.SourceRef)"
			"Source-Commit: $($provenance.SourceCommit)"
			"Source-Path: $($provenance.Path)"
			('Source-Path-Commit: ' + $(if ($provenance.PathCommit) { $provenance.PathCommit } else { '(none)' }))
		)

		if ($provenance.AuthorName -and $provenance.AuthorEmail) {
			$trailers += "Co-authored-by: $($provenance.AuthorName) <$($provenance.AuthorEmail)>"
		}

		$filesList = "* $path"

		$sliceMsg = @"
Mergetopus - slice$sliceNumber from $MergeSource (theirs)

Files:
$filesList

$($trailers -join "`n")
"@

		$authorState = Set-TemporaryGitAuthor -Name $provenance.AuthorName -Email $provenance.AuthorEmail -Date $provenance.AuthorDate
		try {
			Invoke-Git -Arguments @('commit', '-m', $sliceMsg) | Out-Null
		}
		finally {
			Restore-TemporaryGitAuthor -State $authorState
		}
	}

	$sliceBranches.Add($sliceBranch)
}

# Return user to integration branch.
Invoke-Git -Arguments @('checkout', $integrationBranch) | Out-Null

[pscustomobject]@{
	CurrentBranch     = $currentBranch
	RememberedHead    = $headSha
	MergeBase         = $mergeBaseSha
	MergeSource       = $MergeSource
	SourceCommit      = $sourceSha
	IntegrationBranch = $integrationBranch
	ConflictCount     = $conflictedFiles.Count
	ConflictFiles     = $conflictedFiles
	SliceBranches     = $sliceBranches.ToArray()
}

