<#
.SYNOPSIS
    Creates a synthetic Git repository matching the "Large Team Merge Across LTS Versions"
    example, with both overlapping and non-overlapping changes.

.NOTES
    Requires git in PATH.
#>

[CmdletBinding()]
param(
	[Parameter(Mandatory)]
	[string]$RepositoryPath
)

$ErrorActionPreference = 'Stop'

if (Test-Path $RepositoryPath) {
	throw "Target path already exists: $RepositoryPath"
}

New-Item -ItemType Directory -Path $RepositoryPath -Force | Out-Null
Set-Location $RepositoryPath

git init -b bootstrap

function Write-File {
	param(
		[string]$Path,
		[string]$Content
	)

	$dir = Split-Path $Path -Parent
	if ($dir) {
		New-Item -ItemType Directory -Force -Path $dir | Out-Null
	}

	Set-Content -Path $Path -Value $Content -NoNewline
}

function New-GitCommit {
	param(
		[string]$Author,
		[string]$Email,
		[string]$Message
	)

	git add .

	git `
		-c "user.name=$Author" `
		-c "user.email=$Email" `
		commit --author="$Author <$Email>" -m $Message | Out-Null

	Write-Host "Created: $Message"
}

#
# M0
#

Write-File 'config.toml' @'
[network]
retry_count = 3
cache_enabled = false
'@

Write-File 'engine.rs' @'
pub fn start_engine() {
    println!("engine started");
}
'@

Write-File 'api.rs' @'
pub fn api_version() -> &'static str {
    "1.0"
}
'@

Write-File 'utils.rs' @'
pub fn helper() {}
'@

Write-File 'README.md' @'
# Example Project

Synthetic repository for merge testing.
'@

New-GitCommit `
	-Author 'Project Bootstrap' `
	-Email 'bootstrap@example.com' `
	-Message 'M0 initial baseline'

$M0 = (git rev-parse HEAD).Trim()

#
# Create long-lived branches
#

git branch LTS_v17 $M0
git branch LTS_v32 $M0
git branch LTS_v42 $M0

#
# LTS_v17
#

git checkout LTS_v17 | Out-Null

#
# W1
#

Write-File 'config.toml' @'
[network]
retry_count = 10
cache_enabled = false

[pool]
size = 32
'@

Write-File 'engine.rs' @'
pub fn start_engine() {
    println!("engine started");
    println!("pooling enabled");
}
'@

Write-File 'api.rs' @'
pub fn api_version() -> &'static str {
    "1.1"
}
'@

Write-File 'utils.rs' @'
pub fn helper() {}

pub fn pool_helper() {}

pub fn pool_capacity() -> usize {
    32
}
'@

Write-File 'pool.rs' @'
pub struct ConnectionPool {
    size: usize,
}

impl ConnectionPool {
    pub fn new() -> Self {
        Self { size: 32 }
    }
}
'@

Write-File 'docs/pooling.md' @'
# Pooling

Connection pooling support.
'@

New-GitCommit `
	-Author 'Wendy Corduroy' `
	-Email 'wendy@example.com' `
	-Message 'W1 v17 pooling improvements'

#
# G1
#

Write-File 'config.toml' @'
[network]
retry_count = 10
cache_enabled = false
hardening = true

[pool]
size = 32
'@

Write-File 'engine.rs' @'
pub fn start_engine() {
    println!("engine started");
    println!("pooling enabled");
    println!("hardening enabled");
}
'@

Write-File 'security.rs' @'
pub fn validate_configuration() -> bool {
    true
}
'@

Write-File 'docs/threat_model.md' @'
# Threat Model

- Validate startup configuration
- Reject unsafe defaults
- Harden initialization
'@

New-GitCommit `
	-Author 'Gideon Gleeful' `
	-Email 'gideon@example.com' `
	-Message 'G1 v17 hardening'

#
# LTS_v32
#

git checkout LTS_v32 | Out-Null

#
# W2
#

Write-File 'config.toml' @'
[network]
retry_count = 8
cache_enabled = false
'@

Write-File 'retry.rs' @'
pub fn max_retries() -> u32 {
    8
}
'@

Write-File 'backoff.rs' @'
pub fn backoff_ms(attempt: u32) -> u64 {
    attempt as u64 * 100
}
'@

Write-File 'docs/retries.md' @'
# Retry Strategy

Exponential-ish retry behavior.
'@

New-GitCommit `
	-Author 'Wendy Corduroy' `
	-Email 'wendy@example.com' `
	-Message 'W2 v32 retry improvements'

#
# G2
#

Write-File 'config.toml' @'
[network]
retry_count = 8
cache_enabled = true
'@

Write-File 'cache.rs' @'
use std::collections::HashMap;

pub struct Cache {
    data: HashMap<String, String>,
}
'@

Write-File 'tests/cache_tests.rs' @'
#[test]
fn cache_starts_empty() {
    assert!(true);
}
'@

New-GitCommit `
	-Author 'Gideon Gleeful' `
	-Email 'gideon@example.com' `
	-Message 'G2 v32 caching support'

#
# D1
#

Write-File 'engine.rs' @'
pub fn start_engine() {
    println!("engine started");
    println!("debug logging enabled");
}
'@

Write-File 'logging.rs' @'
pub fn debug(message: &str) {
    println!("[DEBUG] {}", message);
}
'@

Write-File 'debug.rs' @'
pub fn dump_state() {}
'@

New-GitCommit `
	-Author 'Dipper Pines' `
	-Email 'dipper@example.com' `
	-Message 'D1 v32 debug logging'

#
# MB1
#

Write-File 'engine.rs' @'
pub fn start_engine() {
    println!("engine started");
    println!("debug logging enabled");
    println!("auth enabled");
}
'@

Write-File 'auth.rs' @'
pub fn authenticate(user: &str) -> bool {
    !user.is_empty()
}
'@

Write-File 'permissions.rs' @'
pub enum Permission {
    Read,
    Write,
}
'@

Write-File 'docs/auth.md' @'
# Authentication

Initial authentication support.
'@

New-GitCommit `
	-Author 'Mabel Pines' `
	-Email 'mabel@example.com' `
	-Message 'MB1 v32 auth and logging'

#
# main
#

git checkout -b main $M0 | Out-Null

#
# DM1
#

Write-File 'metrics.rs' @'
pub fn record_metric(name: &str) {
    println!("{}", name);
}
'@

Write-File 'prometheus.rs' @'
pub fn export_metrics() {}
'@

Write-File 'metrics.toml' @'
enabled = true
'@

New-GitCommit `
	-Author 'Dipper Pines' `
	-Email 'dipper@example.com' `
	-Message 'DM1 metrics'

#
# MM1
#

Write-File 'telemetry.rs' @'
pub fn emit_event(event: &str) {
    println!("{}", event);
}
'@

Write-File 'events.rs' @'
pub struct Event;
'@

Write-File 'telemetry.toml' @'
enabled = true
'@

New-GitCommit `
	-Author 'Mabel Pines' `
	-Email 'mabel@example.com' `
	-Message 'MM1 telemetry'

#
# LTS_v42
#

git checkout LTS_v42 | Out-Null

#
# S1
#

Write-File 'lts42.toml' @'
lts_version = 42
'@

Write-File 'release_notes.md' @'
# LTS v42

Initial integration branch.
'@

Write-File 'migration.rs' @'
pub fn migrate() {}
'@

Write-File 'integration.rs' @'
pub fn integrate() {}
'@

New-GitCommit `
	-Author 'Stan Pines' `
	-Email 'stan@example.com' `
	-Message 'S1 v42 baseline'

Write-Host ''
Write-Host 'Repository created successfully:'
Write-Host "  $RepositoryPath"
Write-Host ''

Write-Host 'Branch topology:'
git log --graph --decorate --oneline --all
