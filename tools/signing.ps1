Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Authenticode signing for release binaries and the installer. Exactly one mode is chosen from the
# environment:
#
#   Azure Artifact Signing (formerly Trusted Signing):
#     FASTPAD_SIGNING_DLIB      path to Azure.CodeSigning.Dlib.dll (x64) from the
#                               Microsoft.ArtifactSigning.Client NuGet package
#     FASTPAD_SIGNING_METADATA  path to the JSON naming Endpoint, CodeSigningAccountName and
#                               CertificateProfileName
#     Credentials come from the Azure identity environment (AZURE_CLIENT_ID, AZURE_TENANT_ID,
#     AZURE_CLIENT_SECRET, or an `az login` session).
#
#   Local certificate:
#     FASTPAD_SIGNING_CERTIFICATE  a certificate SHA-1 thumbprint in the user store, or a PFX path
#     FASTPAD_SIGNING_PASSWORD     optional PFX password
#
# FASTPAD_TIMESTAMP_URL overrides the RFC 3161 timestamp server (Artifact Signing defaults to
# Microsoft's; a local certificate is timestamped only when it is set).

$ArtifactSigningTimestampUrl = "http://timestamp.acs.microsoft.com"

. (Join-Path $PSScriptRoot "msvc.ps1")

function Test-CodeSigningConfigured {
    return [bool]($env:FASTPAD_SIGNING_DLIB -or $env:FASTPAD_SIGNING_CERTIFICATE)
}

function Get-CodeSigningArguments {
    if ($env:FASTPAD_SIGNING_DLIB) {
        if ($env:FASTPAD_SIGNING_CERTIFICATE) {
            throw "Set either FASTPAD_SIGNING_DLIB or FASTPAD_SIGNING_CERTIFICATE, not both."
        }
        if (-not (Test-Path -LiteralPath $env:FASTPAD_SIGNING_DLIB -PathType Leaf)) {
            throw "FASTPAD_SIGNING_DLIB '$env:FASTPAD_SIGNING_DLIB' does not exist."
        }
        if (-not $env:FASTPAD_SIGNING_METADATA -or
            -not (Test-Path -LiteralPath $env:FASTPAD_SIGNING_METADATA -PathType Leaf)) {
            throw "FASTPAD_SIGNING_METADATA must name the Artifact Signing metadata JSON file."
        }
        $timestamp = if ($env:FASTPAD_TIMESTAMP_URL) { $env:FASTPAD_TIMESTAMP_URL } else { $ArtifactSigningTimestampUrl }
        return @("sign", "/fd", "SHA256", "/tr", $timestamp, "/td", "SHA256",
            "/dlib", $env:FASTPAD_SIGNING_DLIB, "/dmdf", $env:FASTPAD_SIGNING_METADATA)
    }

    $arguments = @("sign", "/fd", "SHA256")
    if ($env:FASTPAD_SIGNING_CERTIFICATE -match '^[0-9A-Fa-f]{40}$') {
        $arguments += @("/sha1", $env:FASTPAD_SIGNING_CERTIFICATE)
    }
    elseif (Test-Path -LiteralPath $env:FASTPAD_SIGNING_CERTIFICATE -PathType Leaf) {
        $arguments += @("/f", $env:FASTPAD_SIGNING_CERTIFICATE)
        if ($env:FASTPAD_SIGNING_PASSWORD) {
            $arguments += @("/p", $env:FASTPAD_SIGNING_PASSWORD)
        }
    }
    else {
        throw "FASTPAD_SIGNING_CERTIFICATE must be a certificate SHA-1 thumbprint or a PFX file path."
    }
    if ($env:FASTPAD_TIMESTAMP_URL) {
        $arguments += @("/tr", $env:FASTPAD_TIMESTAMP_URL, "/td", "SHA256")
    }
    return $arguments
}

function Invoke-CodeSigning {
    param([Parameter(Mandatory = $true)] [string[]]$Paths)

    $signTool = Find-SignTool
    $arguments = Get-CodeSigningArguments
    & $signTool @arguments @Paths
    if ($LASTEXITCODE -ne 0) {
        throw "signtool sign failed."
    }
    & $signTool verify /pa @Paths
    if ($LASTEXITCODE -ne 0) {
        throw "signtool verify failed after signing."
    }
}

# The same signtool invocation as an Inno Setup sign tool command, which substitutes $f with the file
# to sign and $q with a double quote.
function Get-InnoSignToolCommand {
    $parts = @(Find-SignTool) + @(Get-CodeSigningArguments) | ForEach-Object {
        if ($_ -match '[\s"]') { '$q' + $_.Replace('"', '') + '$q' } else { $_ }
    }
    return (($parts -join " ") + ' $f')
}
