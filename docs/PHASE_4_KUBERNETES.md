# Phase 4: Kubernetes Orchestration and High Availability

The Kustomize base in `k8s/base/` deploys the Actix API, Redis and Sentinel, a three-instance CloudNativePG PostgreSQL cluster with persistent storage and synchronous replication, an HPA, and NGINX Ingress. Install the CloudNativePG operator before applying the Kustomization. Overlays provide Minikube and production settings; the request-rate overlay enables custom-metric autoscaling when Prometheus Adapter is installed. The checked-in API image (`sayangcare-api:dev`) is configured for local Minikube. Use a registry image for another cluster.

See the [CloudNativePG installation guide](https://cloudnative-pg.io/docs/1.30/installation_upgrade/) and [replication guide](https://cloudnative-pg.io/docs/1.30/replication/) for operator lifecycle, synchronous replication, and production scheduling details.

## Local Minikube Setup

The Minikube overlay limits CloudNativePG `smartShutdownTimeout` to five
seconds (base: 45). Persistent API connections can otherwise hold smart
shutdown open during primary deletion. After this interval CNPG uses fast
shutdown, terminating connections; it is not an immediate/unclean shutdown.
The 90-second total stop/WAL-flush allowance and required synchronous replica
acknowledgment are unchanged. This only targets graceful pod-deletion recovery,
not node/network failures or zero-error availability. For planned maintenance,
switch the primary before deleting it. See
[CNPG shutdown control](https://cloudnative-pg.io/docs/1.28/instance_manager/#shutdown-control).

The local workflow runs simulated calls without a public hostname, TLS certificate, or real Twilio credentials. The external-cluster sections below are optional for this demo. Docker builds the Rust application inside the image, so a separate host Rust installation is not required.

### Windows prerequisites (first time only)

1. Install [Docker Desktop for Windows](https://docs.docker.com/desktop/setup/install/windows-install/), using its WSL 2 backend and Linux containers. Restart Windows if prompted, open Docker Desktop, and wait for the engine to run.
2. Open **PowerShell as Administrator** and install Chocolatey using the command from the [official installation page](https://chocolatey.org/install). Review the remote script before executing it:

```powershell
Set-ExecutionPolicy Bypass -Scope Process -Force
[System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
iex ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
```

3. Close and reopen Administrator PowerShell so the updated PATH is available, then install the Kubernetes tools:

```powershell
choco --version
choco install minikube kubernetes-cli -y
```

4. Close and reopen PowerShell again. Use a normal, non-administrator terminal for the remaining project commands:

```powershell
docker version
minikube version
kubectl version --client
Set-Location "C:\Users\darre\CSC3104-SayangCare Project\CSC3104-SayangCare"
```

`docker version` must show both Client and Server information. If it reports a missing `dockerDesktopLinuxEngine` pipe, start Docker Desktop and wait for its engine before continuing.

### Start the cluster

Run the remaining project commands from the repository root. Start Docker Desktop first, then start the Docker-backed cluster.

**macOS (Terminal):**

```sh
brew install minikube
minikube start --driver=docker
kubectl config current-context
kubectl get nodes
```

**Windows (PowerShell):**

```powershell
minikube start --driver=docker
kubectl config current-context
kubectl get nodes
```

Enable the local storage provisioner, Metrics Server, and NGINX Ingress controller. These commands work in either shell:

```sh
minikube addons enable storage-provisioner
minikube addons enable metrics-server
minikube addons enable ingress
kubectl get storageclass
kubectl top nodes
```

Metrics Server needs time to collect its first samples. If `kubectl top nodes` initially reports that metrics are unavailable, wait one or two minutes and retry.

Build and load the API image. Repeat after changing API code:

```sh
docker build -t sayangcare-api:dev .
minikube image load sayangcare-api:dev
```

Apply the local overlay to create the namespace, ConfigMap, and workloads. Create a test-only Secret first; the placeholder Twilio values are for simulated calls only. This single-line command works in PowerShell and macOS Terminal:

```sh
kubectl apply -f k8s/base/namespace.yaml
kubectl -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD='local-dev-password' --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID='ACtest' --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN='test' --dry-run=client -o yaml | kubectl apply -f -
kubectl -n sayangcare create secret generic sayangcare-db-app --from-literal=username=sayangcare --from-literal=password='local-dev-password' --dry-run=client -o yaml | kubectl apply -f -
```

Install the pinned CloudNativePG operator release, then wait for it to be ready:

```sh
kubectl apply --server-side -f https://raw.githubusercontent.com/cloudnative-pg/cloudnative-pg/release-1.30/releases/cnpg-1.30.1.yaml
kubectl rollout status deployment/cnpg-controller-manager -n cnpg-system
```

Apply the Minikube overlay. Its API init container waits for Redis Sentinel and the PostgreSQL primary before starting:

```sh
kubectl apply -k k8s/overlays/minikube
kubectl -n sayangcare wait --for=condition=Ready cluster/sayangcare-db --timeout=10m
kubectl -n sayangcare rollout status statefulset/redis
kubectl -n sayangcare rollout status statefulset/redis-sentinel
kubectl -n sayangcare rollout status deployment/sayangcare-api
```

Inspect pods, PVCs, service endpoints, metrics, and HPA state:

```sh
kubectl -n sayangcare get pods,svc,pvc,hpa
kubectl -n sayangcare get cluster sayangcare-db
kubectl -n sayangcare get endpointslices -l kubernetes.io/service-name=sayangcare-api
kubectl -n sayangcare top pods
kubectl -n sayangcare describe hpa sayangcare-api
kubectl -n sayangcare logs deployment/sayangcare-api
```

Port-forward the API Service in a separate terminal and leave it running. Use local port **18080** to avoid services such as Apache already listening on port 8080:

```sh
kubectl -n sayangcare port-forward --address 127.0.0.1 svc/sayangcare-api 18080:80
```

Wait for `Forwarding from 127.0.0.1:18080 -> 8080`. In another terminal, check health and open the Call Lab at `http://127.0.0.1:18080/dashboard`:

```sh
curl -i http://127.0.0.1:18080/api/v1/health
curl -i http://127.0.0.1:18080/api/v1/ready
curl http://127.0.0.1:18080/api/v1/metrics
```

In Windows PowerShell, use `curl.exe` rather than the `curl` alias:

```powershell
curl.exe --noproxy "*" -i http://127.0.0.1:18080/api/v1/health
curl.exe --noproxy "*" -i http://127.0.0.1:18080/api/v1/ready
curl.exe --noproxy "*" http://127.0.0.1:18080/api/v1/metrics
```

Health should return HTTP 200; readiness should report Redis and PostgreSQL as `ok`; metrics should include `sayangcare_http_requests_total`.

Expected readiness JSON (key order may vary):

```json
{"dependencies":{"postgres":"ok","redis":"ok"},"ready":true}
```

### Troubleshooting and restarting

- **HTML 404 with `Server: Apache`:** the request reached another HTTP server, not SayangCare. Use the port-forward and matching `18080` URLs above. Do not stop unrelated services just to free port 8080.
- **Port-forward cannot bind:** choose another unused local port (for example `18081:80`) and use that port in every browser and curl URL. Confirm the forwarding message before testing.
- **Connection refused to the Kubernetes API:** check `docker info`, start Docker Desktop if needed, then run `minikube start --driver=docker` and `kubectl get nodes`. Existing workloads can take a few minutes to become ready after a restart.
- **Pods are not ready:** run `kubectl -n sayangcare get pods`, `kubectl -n sayangcare describe pod POD_NAME`, and `kubectl -n sayangcare logs POD_NAME`. Wait for Redis, Sentinel, and PostgreSQL before diagnosing API startup.
- **PostgreSQL Cluster resource is unknown:** install the CloudNativePG operator and wait for its deployment before applying the Kustomize overlay.
- **HPA request-rate metric is unknown:** the Minikube overlay alone supports CPU/memory autoscaling. Request-rate autoscaling requires Prometheus and Prometheus Adapter; install them using the optional monitoring steps below before applying the request-rate overlay.

To stop the demo, press `Ctrl+C` in the port-forward terminal and run `minikube stop`. Resume later by opening Docker Desktop, running `minikube start --driver=docker`, waiting for the workloads, and starting the port-forward again. Do not use `minikube delete` unless you intend to remove the cluster and its local data.

## Local Cluster Results

Rechecked on 21 October 2026 on this Windows machine, after resuming the existing Minikube profile. These checks validate the running deployment and existing image, not a fresh installation or image rebuild.

| Check | Observed result |
| --- | --- |
| Minikube node | `Ready`, single-node Docker-backed cluster |
| PostgreSQL | 3/3 CloudNativePG instances ready; one primary and two streaming standbys |
| Redis / Sentinel | 3/3 pods ready for each StatefulSet |
| API Deployment | 3/3 replicas ready and available |
| Persistent storage | All ten PVCs `Bound` (three PostgreSQL HA volumes, three Redis, three Sentinel, and the retained legacy PostgreSQL volume) |
| API Service discovery | EndpointSlice contained three API pod addresses |
| `GET /api/v1/health` | HTTP 200, service `sayangcare`, status `ok` |
| `GET /api/v1/ready` | HTTP 200, `ready: true`, Redis and PostgreSQL both `ok` |
| `GET /api/v1/metrics` | HTTP 200, `sayangcare_http_requests_total` present |
| `GET /dashboard` | HTTP 200, SayangCare page content confirmed |
| Metrics Server | Available; node and pod CPU/memory samples returned |
| HPA resource metrics | Metrics Server returned CPU and memory samples; baseline targets are 65% CPU and 75% memory |
| Custom request-rate metric | Prometheus Adapter API was available; HPA target is 25 requests/second per API pod |
| Ingress / external route | Public HTTPS health, readiness, and dashboard returned HTTP 200; unsigned Twilio POST returned HTTP 403 |
| Redis failover | Sentinel promoted another Redis node in about 7 seconds; API reads and readiness recovered about 8 seconds after fault injection, and the old master rejoined as a replica |
| API pod replacement | 120/120 polled session requests succeeded while an API pod was deleted; Deployment returned to 3/3 ready in about 10.4 seconds |
| CPU/memory autoscaling | A temporary 20% CPU target scaled 3 to 5 replicas; the configured 65% CPU target did not scale under the test load, which reached about 28–34% CPU |
| Request-rate autoscaling | 48 in-cluster load workers drove measured per-pod rates above 25 requests/second; the HPA scaled the API from 3 to 6 replicas, then returned to 3/3 after load removal |
| PostgreSQL data migration | Existing database was backed up/restored; row counts for sessions, escalations, volunteers, and assignments matched |
| PostgreSQL replication | Two streaming replicas reported quorum synchronous status; API readiness returned HTTP 200 against the new primary |
| PostgreSQL pod failover | With a 45-second smart-shutdown window, the replacement primary was selected in about 58 seconds. One API readiness poll returned HTTP 503 at 51 seconds; the next returned HTTP 200 at 58 seconds. All three database instances recovered in about 139 seconds, and the session row count remained 3. This is a pod-failure demo, not an availability guarantee. |

HTTP checks used temporary port-forwards and an external HTTPS tunnel; the tunnel was stopped after validation. The Minikube cluster and monitoring stack remain deployed. The test did not place a real signed Twilio call or validate production DNS/TLS. A single-node cluster cannot test Kubernetes node-level high availability.

The existing single-instance PostgreSQL data was exported and restored into the CloudNativePG cluster before the API was switched to its read/write Service. The old PVC was retained, and the old PostgreSQL StatefulSet was scaled down as a rollback copy. When migrating another populated deployment, take and verify a database backup before switching the API; applying the Kustomize overlay alone does not copy data between PostgreSQL clusters.

### Enable request-rate autoscaling

The CPU/memory HPA works with Metrics Server alone. To enable the optional request-rate metric, install Prometheus and Prometheus Adapter with the checked-in local values, then apply the request-rate overlay:

```sh
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo update
helm upgrade --install prometheus prometheus-community/prometheus --namespace monitoring --create-namespace --values k8s/monitoring/prometheus-values.yaml
helm upgrade --install prometheus-adapter prometheus-community/prometheus-adapter --namespace monitoring --values k8s/monitoring/prometheus-adapter-values.yaml
kubectl apply -k k8s/overlays/request-rate-hpa
kubectl get apiservice v1beta1.custom.metrics.k8s.io
kubectl -n sayangcare describe hpa sayangcare-api
```

The request-rate overlay keeps the CPU and memory targets and adds a target of 25 requests/second per pod. The test uses generated HTTP traffic; tune the target for the workload before production use.

## API Pod Recovery Test

Create a simulated call in the dashboard and note its session ID. Delete one API pod and wait for the Deployment to replace it:

```sh
kubectl -n sayangcare get pods -l app=sayangcare-api
kubectl -n sayangcare delete pod POD_NAME
kubectl -n sayangcare rollout status deployment/sayangcare-api
kubectl -n sayangcare get pods -l app=sayangcare-api
```

Reload the session in the dashboard or request `GET /api/v1/calls/SESSION_ID` through the port-forward. The surviving API pod should retrieve the shared session from Redis. This validates a later request after pod loss, not an HTTP/Twilio request that was already in flight. Save the session ID and command output if using this as project evidence.

A Service port-forward selects one pod; it is not a load balancer across all replicas. If you delete the selected pod, the forwarding process can exit. Start the port-forward again before retrying the session request; a closed local tunnel alone is not evidence that shared session recovery failed.

## PostgreSQL Primary Failover Test

CloudNativePG creates a read/write Service that follows the promoted primary. To test pod-level recovery, record the current primary, delete that pod, and monitor the Cluster status and API readiness:

```sh
kubectl -n sayangcare get cluster sayangcare-db -o jsonpath='{.status.currentPrimary}'
kubectl -n sayangcare delete pod PRIMARY_POD_NAME
kubectl -n sayangcare get cluster sayangcare-db -w
kubectl -n sayangcare get pods -l cnpg.io/cluster=sayangcare-db -w
```

During promotion, database connections can be briefly unavailable, so API readiness may return HTTP 503. Wait for `readyInstances` to equal `3`, then check `GET /api/v1/ready` and verify application rows on the new primary. This only tests instance/pod failover; it does not test loss of the Kubernetes node or off-cluster backup recovery.

## External Cluster Prerequisites

A non-local cluster needs a working `kubectl` context, the CloudNativePG operator, a default `ReadWriteOnce` StorageClass, Metrics Server, Prometheus scraping the API pod annotations, Prometheus Adapter for request-rate metrics, and an NGINX Ingress Controller with the `nginx` IngressClass. Place database instances on separate nodes (or zones) to survive node loss. Public Twilio traffic also needs a real hostname, DNS pointing at the Ingress controller, and a valid TLS Secret named `sayangcare-tls` in namespace `sayangcare`.

For request-rate HPA support, replace the Prometheus service URL in `docs/prometheus-adapter-values.yaml` and install the adapter. This is a Helm values file, not a Kubernetes manifest:

```sh
helm upgrade --install prometheus-adapter prometheus-community/prometheus-adapter --namespace monitoring --create-namespace --values docs/prometheus-adapter-values.yaml
```

Verify Prometheus collects `sayangcare_http_requests_total` and that `sayangcare_http_requests_per_second` appears in the Kubernetes custom metrics API. The HPA targets CPU at 65%, memory at 75%, and 25 requests per second per pod; load-test and tune these initial targets. Without Metrics Server, CPU/memory scaling is unavailable; without Prometheus Adapter, request-rate scaling is unavailable.

## Registry Image, Secrets, and TLS

Build and push a registry image for remote clusters, replacing the example registry owner:

```sh
docker build -t ghcr.io/REPLACE_WITH_GITHUB_OWNER/sayangcare-api:v0.1.0 .
docker push ghcr.io/REPLACE_WITH_GITHUB_OWNER/sayangcare-api:v0.1.0
```

Set that image in `k8s/overlays/production/kustomization.yaml`. For a private registry, create an `imagePullSecret` and reference it from the API pod spec.

Create the namespace and `sayangcare-secrets` using a secret manager or shell variables. Do not commit real credentials. macOS/Linux shell example:

```sh
kubectl apply -f k8s/base/namespace.yaml
kubectl -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD="$POSTGRES_PASSWORD" --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID="$SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID" --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN="$SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN"
kubectl -n sayangcare create secret generic sayangcare-db-app --from-literal=username=sayangcare --from-literal=password="$POSTGRES_PASSWORD"
```

Windows PowerShell equivalent:

```powershell
kubectl -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD="$env:POSTGRES_PASSWORD" --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID="$env:SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID" --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN="$env:SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN"
kubectl -n sayangcare create secret generic sayangcare-db-app --from-literal=username=sayangcare --from-literal=password="$env:POSTGRES_PASSWORD"
```

Create the CloudNativePG bootstrap application Secret with the **same** PostgreSQL password. Keep the credentials in your secret manager or environment; do not commit them:

```sh
kubectl -n sayangcare create secret generic sayangcare-db-app --from-literal=username=sayangcare --from-literal=password="$POSTGRES_PASSWORD"
```

In PowerShell, use `--from-literal=password="$env:POSTGRES_PASSWORD"`.

Update the example host and TLS settings in `k8s/overlays/production/kustomization.yaml`, and set `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL` in `k8s/base/namespace.yaml` to the same HTTPS URL. Create the TLS Secret from an issued certificate and key:

```sh
kubectl -n sayangcare create secret tls sayangcare-tls --cert=/path/to/fullchain.pem --key=/path/to/private-key.pem
```

On Windows, pass Windows certificate paths:

```powershell
kubectl -n sayangcare create secret tls sayangcare-tls --cert="C:\certs\fullchain.pem" --key="C:\certs\private-key.pem"
```

After the required secrets, hostname, DNS, and TLS are ready, deploy and verify:

```sh
kubectl apply --server-side -f https://raw.githubusercontent.com/cloudnative-pg/cloudnative-pg/release-1.30/releases/cnpg-1.30.1.yaml
kubectl rollout status deployment/cnpg-controller-manager -n cnpg-system
kubectl apply -k k8s/overlays/production
kubectl -n sayangcare wait --for=condition=Ready cluster/sayangcare-db --timeout=10m
kubectl -n sayangcare rollout status statefulset/redis
kubectl -n sayangcare rollout status statefulset/redis-sentinel
kubectl -n sayangcare rollout status deployment/sayangcare-api
kubectl -n sayangcare get pods,svc,pvc,ingress,hpa
```

Point the Twilio voice webhook to `https://YOUR_HOST/twilio/voice`. The configured `PUBLIC_BASE_URL` must match the externally visible URL so Twilio signature verification uses the right URL. Do not apply the production overlay until its image, host, secrets, DNS, and TLS values are configured.

## Reliability Limits

CloudNativePG manages three PostgreSQL instances and PVCs; the API connects to the operator-managed read/write Service so it follows the promoted primary. One synchronous standby is required before a write is acknowledged, favoring durability over write availability if no standby is healthy. The 45-second smart-shutdown setting allows active work a brief drain window before failover. In the pod deletion test, a new primary was selected in about 58 seconds, one readiness poll returned HTTP 503, and the full three-instance cluster recovered in about 139 seconds. The single-node Minikube cluster cannot demonstrate node-level high availability, and database replication is not a backup. Configure and test off-cluster backups and restores before production use. Redis and Sentinel also have three StatefulSet pods, PVCs, node-spread preferences, and disruption budgets. The API session store, queue, and circuit breaker refresh Sentinel master discovery at most once per second; a command already in flight during Redis promotion can still fail and is not automatically replayed.

This is a learning deployment, not a production security baseline. Add Redis authentication, network policies, managed secret delivery, backups, and tested restore procedures before handling real caller data.
