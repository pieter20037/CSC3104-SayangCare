# Phase 4: Kubernetes Orchestration and High Availability

The manifests in `k8s/` deploy the Actix API, Redis and Sentinel, PostgreSQL with persistent storage, an HPA, and NGINX Ingress. The checked-in API image (`sayangcare-api:dev`) is configured for the local Minikube setup. Use a registry image when deploying to another cluster.

## Local Minikube Setup

Start Docker Desktop first. Install Minikube with the package manager for your OS, then start a Docker-backed cluster.

**macOS (Terminal):**

```sh
brew install minikube
minikube start --driver=docker
kubectl config current-context
kubectl get nodes
```

**Windows (PowerShell):** install Minikube using Chocolatey (`choco install minikube`) or its official Windows installer, then run:

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

Build and load the API image. Repeat after changing API code:

```sh
docker build -t sayangcare-api:dev .
minikube image load sayangcare-api:dev
```

Apply the namespace and ConfigMap, then create a test-only Secret. The placeholder Twilio values are for simulated calls only. This single-line command works in PowerShell and macOS Terminal:

```sh
kubectl apply -f k8s/namespace.yaml
kubectl -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD='local-dev-password' --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID='ACtest' --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN='test' --dry-run=client -o yaml | kubectl apply -f -
```

Deploy stateful dependencies before the API. API startup exits if Redis or PostgreSQL is unavailable at bootstrap:

```sh
kubectl apply -f k8s/postgres.yaml -f k8s/redis-sentinel.yaml
kubectl -n sayangcare rollout status statefulset/postgres
kubectl -n sayangcare rollout status statefulset/redis
kubectl -n sayangcare rollout status statefulset/redis-sentinel

kubectl apply -f k8s/app-deployment.yaml -f k8s/hpa.yaml
kubectl -n sayangcare rollout status deployment/sayangcare-api
```

Inspect pods, PVCs, service endpoints, metrics, and HPA state:

```sh
kubectl -n sayangcare get pods,svc,pvc,hpa
kubectl -n sayangcare get endpoints sayangcare-api
kubectl -n sayangcare top pods
kubectl -n sayangcare describe hpa sayangcare-api
kubectl -n sayangcare logs deployment/sayangcare-api
```

Port-forward the API Service:

```sh
kubectl -n sayangcare port-forward svc/sayangcare-api 8080:80
```

In another terminal, check health and open the Call Lab at `http://127.0.0.1:8080/dashboard`:

```sh
curl -i http://127.0.0.1:8080/api/v1/health
curl -i http://127.0.0.1:8080/api/v1/ready
curl http://127.0.0.1:8080/api/v1/metrics
```

In Windows PowerShell, use `curl.exe` rather than the `curl` alias:

```powershell
curl.exe -i http://127.0.0.1:8080/api/v1/health
curl.exe -i http://127.0.0.1:8080/api/v1/ready
curl.exe http://127.0.0.1:8080/api/v1/metrics
```

Health should return HTTP 200; readiness should report Redis and PostgreSQL as `ok`; metrics should include `sayangcare_http_requests_total`.

## Local Cluster Results

On 15 October 2026, the single-node Docker-backed Minikube cluster was `Ready`. PostgreSQL was `1/1`, Redis `3/3`, Sentinel `3/3`, and API `3/3`; their PVCs were `Bound`, and the API Service had three endpoints. `/api/v1/ready` reported Redis and PostgreSQL healthy. Metrics Server returned API pod samples and the HPA showed CPU/memory utilization. The custom request-rate metric was not available because Prometheus Adapter/custom metrics API had not been installed. The Ingress addon was enabled, but external DNS, TLS, and Twilio routing were not tested. Redis master failover and API-pod deletion during an active simulated call remain unverified. A single-node cluster cannot test Kubernetes node-level high availability.

## API Pod Recovery Test

Create a simulated call in the dashboard and note its session ID. Delete one API pod and wait for the Deployment to replace it:

```sh
kubectl -n sayangcare get pods -l app=sayangcare-api
kubectl -n sayangcare delete pod POD_NAME
kubectl -n sayangcare rollout status deployment/sayangcare-api
kubectl -n sayangcare get pods -l app=sayangcare-api
```

Reload the session in the dashboard or request `GET /api/v1/calls/SESSION_ID` through the port-forward. The surviving API pod should retrieve the shared session from Redis. This validates a later request after pod loss, not an HTTP/Twilio request that was already in flight. Save the session ID and command output if using this as project evidence.

## External Cluster Prerequisites

A non-local cluster needs a working `kubectl` context, a default `ReadWriteOnce` StorageClass, Metrics Server, Prometheus scraping the API pod annotations, Prometheus Adapter for request-rate metrics, and an NGINX Ingress Controller with the `nginx` IngressClass. Public Twilio traffic also needs a real hostname, DNS pointing at the Ingress controller, and a valid TLS Secret named `sayangcare-tls` in namespace `sayangcare`.

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

Set that image in `k8s/app-deployment.yaml`. For a private registry, create an `imagePullSecret` and reference it from the API pod spec.

Create `sayangcare-secrets` using a secret manager or shell variables. Do not commit real credentials. macOS/Linux shell example:

```sh
kubectl -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD="$POSTGRES_PASSWORD" --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID="$SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID" --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN="$SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN"
```

Windows PowerShell equivalent:

```powershell
kubectl -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD="$env:POSTGRES_PASSWORD" --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID="$env:SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID" --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN="$env:SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN"
```

Update the example host in `k8s/ingress.yaml` and `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL` in `k8s/namespace.yaml` to the same HTTPS URL. Create the TLS Secret from an issued certificate and key:

```sh
kubectl -n sayangcare create secret tls sayangcare-tls --cert=/path/to/fullchain.pem --key=/path/to/private-key.pem
```

On Windows, pass Windows certificate paths:

```powershell
kubectl -n sayangcare create secret tls sayangcare-tls --cert="C:\certs\fullchain.pem" --key="C:\certs\private-key.pem"
```

After the required secrets, hostname, DNS, and TLS are ready, deploy and verify:

```sh
kubectl apply -f k8s/
kubectl -n sayangcare rollout status statefulset/postgres
kubectl -n sayangcare rollout status statefulset/redis
kubectl -n sayangcare rollout status statefulset/redis-sentinel
kubectl -n sayangcare rollout status deployment/sayangcare-api
kubectl -n sayangcare get pods,svc,pvc,ingress,hpa
```

Point the Twilio voice webhook to `https://YOUR_HOST/twilio/voice`. The configured `PUBLIC_BASE_URL` must match the externally visible URL so Twilio signature verification uses the right URL.

## Reliability Limits

PostgreSQL has a single replica. Its PVC preserves data across pod replacement but does not provide database failover. Redis and Sentinel have three StatefulSet pods, PVCs, node-spread preferences, and disruption budgets. The API session store, queue, and circuit breaker refresh Sentinel master discovery at most once per second; a command already in flight during promotion can still fail and is not automatically replayed. Exercise Sentinel failover before claiming end-to-end Redis HA.

This is a learning deployment, not a production security baseline. Add Redis authentication, network policies, managed secret delivery, backups, and tested restore procedures before handling real caller data.
