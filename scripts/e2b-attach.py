"""Pinned SDK attachment using GET only; never connect/resume compute.

SandboxInfo intentionally omits envd credentials. Use the SDK's generated detail
reader, check ownership/state on that same response, and keep its access token
inside the private worker. No credential is returned to the coordinator.
"""


def attach_running(runtime, key, labels, sandbox_class):
    from e2b.api.client_sync import get_api_client
    from e2b.api.client.api.sandboxes import get_sandboxes_sandbox_id
    from e2b.api.client.models.sandbox_detail import SandboxDetail
    from e2b.connection_config import ConnectionConfig
    from packaging.version import Version

    config = ConnectionConfig(api_key=key, request_timeout=10, retries=0)
    response = get_sandboxes_sandbox_id.sync_detailed(runtime, client=get_api_client(config))
    detail = response.parsed
    if (response.status_code != 200 or not isinstance(detail, SandboxDetail)
            or detail.sandbox_id != runtime
            or getattr(detail.state, 'value', detail.state) != 'running'
            or not isinstance(detail.metadata, dict)
            or any(detail.metadata.get(name) != value for name, value in labels.items())
            or not isinstance(detail.envd_access_token, str) or not detail.envd_access_token):
        return None
    token = detail.envd_access_token
    guest_config = ConnectionConfig(
        api_key=key, request_timeout=10, retries=0,
        extra_sandbox_headers={'E2b-Sandbox-Id': runtime,
                               'E2b-Sandbox-Port': str(ConnectionConfig.envd_port),
                               'X-Access-Token': token})
    return sandbox_class(sandbox_id=runtime, sandbox_domain=detail.domain if isinstance(detail.domain, str) else None,
                         envd_version=Version(detail.envd_version), envd_access_token=token,
                         traffic_access_token=None, connection_config=guest_config)
