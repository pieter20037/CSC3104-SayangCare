param(
    [Parameter(Mandatory = $true)][int]$DatabaseMaxConnections,
    [int]$ReservedConnections = 16,
    [int]$ExtraTerminatingPods = 1,
    [int]$ProposedPoolPerPod = 0,
    [string]$Namespace = 'sayangcare'
)
$ErrorActionPreference = 'Stop'
if ($DatabaseMaxConnections -le 0 -or $ReservedConnections -lt 0 -or $ExtraTerminatingPods -lt 0) { throw 'Invalid capacity inputs' }
$hpaJson = kubectl get hpa -n $Namespace -o json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read HPA' }
$hpa = ($hpaJson | ConvertFrom-Json).items | Where-Object { $_.spec.scaleTargetRef.name -eq 'sayangcare-api' }
if (@($hpa).Count -ne 1) { throw 'Expected exactly one API HPA' }
$configJson = kubectl get configmap sayangcare-config -n $Namespace -o json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read ConfigMap' }
$config = $configJson | ConvertFrom-Json
$deployJson = kubectl get deployment sayangcare-api -n $Namespace -o json
if ($LASTEXITCODE -ne 0) { throw 'Cannot read Deployment' }
$deploy = $deployJson | ConvertFrom-Json
$pool = [int]$config.data.SAYANGCARE__POSTGRES__MAX_CONNECTIONS
$apiContainer = $deploy.spec.template.spec.containers | Where-Object { $_.name -eq 'api' }
$override = $apiContainer.env | Where-Object { $_.name -eq 'SAYANGCARE__POSTGRES__MAX_CONNECTIONS' }
if ($override) {
    if ($override.valueFrom) { throw 'Pool size comes from valueFrom; resolve it before assessing capacity' }
    $pool = [int]$override.value
}
if ($ProposedPoolPerPod -gt 0) { $pool = $ProposedPoolPerPod }
if ($pool -le 0) { throw 'Cannot determine effective positive pool limit' }
$replicas = [int]$hpa.spec.maxReplicas
$surgeValue = [string]$deploy.spec.strategy.rollingUpdate.maxSurge
if (-not $surgeValue) { $surgeValue = '25%' }
$surge = if ($surgeValue.EndsWith('%')) { [int][Math]::Ceiling($replicas * [double]$surgeValue.TrimEnd('%') / 100) } else { [int]$surgeValue }
$connections = ($replicas + $surge + $ExtraTerminatingPods) * $pool
$required = $connections + $ReservedConnections
[pscustomobject]@{
    HpaMaxReplicas = $replicas; PoolPerPod = $pool; RolloutSurge = $surge
    ExtraTerminatingPods = $ExtraTerminatingPods; ApiConnections = $connections
    ReservedConnections = $ReservedConnections; Required = $required
    DatabaseMaxConnections = $DatabaseMaxConnections; Fits = ($required -le $DatabaseMaxConnections)
    ProjectionOnly = ($ProposedPoolPerPod -gt 0)
}
if ($required -gt $DatabaseMaxConnections) { throw 'Connection budget exceeded: reduce per-pod pools or HPA ceiling, or increase verified DB capacity' }
