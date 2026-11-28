#!/usr/bin/env python3
"""Check a pinned SDK's public API or drive the default-discovery Gateway route."""
import asyncio
import inspect
import sys
import uuid
from importlib.metadata import version as package_version
from pathlib import Path

SDK_VERSION = sys.argv[1]
BASE_URL = sys.argv[2] if len(sys.argv) > 2 else None
QA_ROOT = Path(sys.argv[3]) if len(sys.argv) > 3 else None


def require(condition, message):
    if not condition:
        raise AssertionError(message)
    print(f"PASS: {message}")


require(package_version("a2a-sdk") == SDK_VERSION,
        f"installed official distribution a2a-sdk=={SDK_VERSION}")


def task_id(value):
    if isinstance(value, tuple):
        value = value[0]
    if hasattr(value, "HasField") and "task" in value.DESCRIPTOR.fields_by_name:
        if value.HasField("task"):
            value = value.task
    return getattr(value, "id", None) or getattr(value, "task_id", None)


def task_state(value):
    status = getattr(value, "status", None)
    if status is not None:
        status = getattr(status, "state", status)
    if isinstance(status, int):
        try:
            from a2a.types import TaskState
            status = TaskState.Name(status)
        except (ImportError, ValueError):
            pass
    return str(getattr(status, "name", status)).lower()


async def wait_for_core_provider(prompt):
    if QA_ROOT is None:
        return
    provider_log = QA_ROOT / "mock-provider.log"
    async with asyncio.timeout(30):
        while (not provider_log.exists() or not any(
            prompt in line and "tools=" in line and "tools=0" not in line
            for line in provider_log.read_text().splitlines()
        )):
            await asyncio.sleep(0.1)
    require(True, "the controlled provider received the Core task before cancellation")


async def verify_remote_clarification(version):
    if QA_ROOT is None:
        return
    import httpx

    v1 = version == "1.0"
    send_method = "SendMessage" if v1 else "message/send"
    cancel_method = "CancelTask" if v1 else "tasks/cancel"
    owner, other = "qa-a2a-owner", "qa-a2a-other"
    async with httpx.AsyncClient(timeout=45) as client:
        async def rpc(method, params, token):
            reply = await client.post(
                f"{BASE_URL}/a2a/rpc",
                headers={"A2A-Version": version, "Authorization": f"Bearer {token}"},
                json={"jsonrpc": "2.0", "id": "clarification", "method": method, "params": params},
            )
            reply.raise_for_status()
            return reply.json()

        async def send(text, token, task=None):
            part = {"text": text}
            message = {"messageId": str(uuid.uuid4()), "role": "ROLE_USER" if v1 else "user",
                       "parts": [part]}
            if not v1:
                message["kind"] = "message"
                part["kind"] = "text"
            if task:
                message.update(taskId=task["id"], contextId=task["contextId"])
            return await rpc(send_method, {"message": message}, token)

        def task_result(reply):
            require("result" in reply, f"{version} clarification RPC succeeds: {reply}")
            return reply["result"]["task"] if v1 else reply["result"]

        paused = task_result(await send("A2A clarification probe", owner))
        expected_pause = "TASK_STATE_INPUT_REQUIRED" if v1 else "input-required"
        require(paused["status"]["state"] == expected_pause, f"{version} Core ask_user pauses the task")
        question = paused["status"]["message"]["parts"][0]["text"]
        require("Which color should I use?" in question, f"{version} task carries the model's question")
        foreign = await send("blue", other, paused)
        require(foreign.get("error", {}).get("code") == -32001,
                f"{version} another credential cannot resume the task")
        resumed = task_result(await send("Use Blue.", owner, paused))
        require(resumed["id"] == paused["id"] and resumed["contextId"] == paused["contextId"],
                f"{version} same-credential follow-up preserves task and context IDs")
        expected_complete = "TASK_STATE_COMPLETED" if v1 else "completed"
        require(resumed["status"]["state"] == expected_complete,
                f"{version} follow-up completes in a new Core run")

        paused = task_result(await send("A2A clarification probe", owner))
        require(paused["status"]["state"] == expected_pause,
                f"{version} second clarification is paused before cancellation")
        cancellation = await rpc(cancel_method, {"id": paused["id"]}, owner)
        expected_cancel = "TASK_STATE_CANCELED" if v1 else "canceled"
        require(cancellation.get("result", {}).get("status", {}).get("state") == expected_cancel,
                f"{version} the owner can cancel a paused task: {cancellation}")


def sdk_03():
    from a2a.client import Client, ClientConfig, ClientFactory, create_text_message_object
    from a2a.types import MessageSendConfiguration, TaskIdParams, TaskQueryParams

    require(all(callable(getattr(Client, name, None)) for name in
                ("send_message", "get_task", "cancel_task", "get_card")),
            "0.3 SDK client exposes send, GetTask, CancelTask, and card operations")
    require(callable(ClientFactory.connect), "0.3 SDK has official URL discovery client")
    require(callable(create_text_message_object), "0.3 SDK exports text message helper")
    require("blocking" in inspect.signature(MessageSendConfiguration).parameters,
            "0.3 SDK exposes blocking send configuration")
    require("id" in inspect.signature(TaskQueryParams).parameters and
            "id" in inspect.signature(TaskIdParams).parameters,
            "0.3 SDK exposes GetTask and CancelTask parameter models")
    if BASE_URL:
        return asyncio.run(run_03(ClientConfig, ClientFactory, create_text_message_object,
                                  MessageSendConfiguration, TaskIdParams, TaskQueryParams))


async def run_03(ClientConfig, ClientFactory, make_message, SendConfig, CancelParams, GetParams):
    client = await ClientFactory.connect(BASE_URL, client_config=ClientConfig(streaming=True))
    card = await client.get_card()
    url = getattr(card, "url", "")
    require("/a2a/rpc" in url, "0.3 default discovery selects advertised JSON-RPC endpoint")
    prompt = "SDK v0.3.26 inbound QA"
    message = make_message(content=prompt)
    stream = client.send_message(message)
    events = []
    first_task = None
    async for event in stream:
        events.append(event)
        first_task = first_task or task_id(event)
        if first_task:
            break
    require(bool(events), "0.3 send opens an SSE task stream")
    require(bool(first_task), "0.3 SSE begins with an identifiable task")
    await stream.aclose()
    await wait_for_core_provider(prompt)
    cancelled = await client.cancel_task(CancelParams(id=first_task))
    require(task_id(cancelled) == first_task, "0.3 CancelTask addresses the live streamed task")
    current = await client.get_task(GetParams(id=first_task))
    require(task_id(current) == first_task, "0.3 GetTask returns the streamed task")
    require("cancel" in task_state(current), "0.3 task status reflects Core cancellation")
    await verify_remote_clarification("0.3")


def sdk_10():
    from a2a.client import A2ACardResolver, Client, ClientConfig, ClientFactory
    from a2a.types import CancelTaskRequest, GetTaskRequest, Message, Part, Role, SendMessageConfiguration, SendMessageRequest

    require(all(callable(getattr(Client, name, None)) for name in
                ("send_message", "get_task", "cancel_task")),
            "1.0 SDK client exposes send, GetTask, and CancelTask operations")
    factory = ClientFactory()
    require(callable(A2ACardResolver.get_agent_card), "1.0 SDK exposes default card discovery")
    require(callable(factory.create), "1.0 SDK creates a transport client from the discovered card")
    require("return_immediately" in SendMessageConfiguration.DESCRIPTOR.fields_by_name,
            "1.0 SDK exposes returnImmediately configuration")
    require("id" in GetTaskRequest.DESCRIPTOR.fields_by_name and
            "id" in CancelTaskRequest.DESCRIPTOR.fields_by_name,
            "1.0 SDK exposes GetTask and CancelTask request models")
    AgentCard = __import__("a2a.types", fromlist=["AgentCard"]).AgentCard
    require("supported_interfaces" in AgentCard.DESCRIPTOR.fields_by_name,
            "1.0 SDK models supportedInterfaces in discovery cards")
    if BASE_URL:
        return asyncio.run(run_10(factory, Message, Part, Role, SendMessageConfiguration,
                                  SendMessageRequest, GetTaskRequest, CancelTaskRequest))


async def run_10(factory, Message, Part, Role, SendConfig, SendRequest, GetRequest, CancelRequest):
    import httpx
    from a2a.client import A2ACardResolver, ClientConfig, ClientFactory

    async with httpx.AsyncClient() as http_client:
        configured_factory = ClientFactory(ClientConfig(httpx_client=http_client))
        card = await A2ACardResolver(http_client, BASE_URL).get_agent_card()
        interfaces = list(getattr(card, "supported_interfaces", []))
        require(any("/a2a/rpc" in item.url and item.protocol_version.startswith("1.0")
                    for item in interfaces), "1.0 default discovery selects advertised JSON-RPC 1.0 endpoint")
        client = configured_factory.create(card)
        prompt = "SDK v1.0.3 inbound QA"
        message = Message(message_id=str(uuid.uuid4()), role=Role.ROLE_USER,
                          parts=[Part(text=prompt)])
        request = SendRequest(message=message,
                              configuration=SendConfig(return_immediately=True))
        stream = client.send_message(request)
        events = []
        first_task = None
        async for event in stream:
            events.append(event)
            first_task = first_task or task_id(event)
            if first_task:
                break
        require(bool(events), "1.0 SendMessage opens an SSE task stream")
        require(bool(first_task), "1.0 SSE begins with an identifiable task")
        await stream.aclose()
        await wait_for_core_provider(prompt)
        cancelled = await client.cancel_task(CancelRequest(id=first_task))
        require(task_id(cancelled) == first_task, "1.0 CancelTask addresses the live streamed task")
        current = await client.get_task(GetRequest(id=first_task))
        require(task_id(current) == first_task, "1.0 GetTask returns the streamed task")
        require("cancel" in task_state(current), "1.0 task status reflects Core cancellation")
    await verify_remote_clarification("1.0")


if SDK_VERSION == "0.3.26":
    sdk_03()
elif SDK_VERSION == "1.0.3":
    sdk_10()
else:
    raise SystemExit(f"unsupported pinned SDK: {SDK_VERSION}")
