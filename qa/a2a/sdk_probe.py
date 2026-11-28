#!/usr/bin/env python3
"""Check a pinned SDK's public API or drive the default-discovery Gateway route."""
import asyncio
import inspect
import sys
import uuid
from importlib.metadata import version as package_version

SDK_VERSION = sys.argv[1]
BASE_URL = sys.argv[2] if len(sys.argv) > 2 else None


def require(condition, message):
    if not condition:
        raise AssertionError(message)
    print(f"PASS: {message}")


require(package_version("a2a-sdk") == SDK_VERSION,
        f"installed official distribution a2a-sdk=={SDK_VERSION}")


def task_id(value):
    if isinstance(value, tuple):
        value = value[0]
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
    message = make_message(content="SDK v0.3.26 inbound QA")
    stream = await client.send_message(message)
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
    cancelled = await client.cancel_task(CancelParams(id=first_task))
    require(task_id(cancelled) == first_task, "0.3 CancelTask addresses the live streamed task")
    current = await client.get_task(GetParams(id=first_task))
    require(task_id(current) == first_task, "0.3 GetTask returns the streamed task")
    require("cancel" in task_state(current), "0.3 task status reflects Core cancellation")


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
        message = Message(message_id=str(uuid.uuid4()), role=Role.ROLE_USER,
                          parts=[Part(text="SDK v1.0.3 inbound QA")])
        request = SendRequest(message=message,
                              configuration=SendConfig(return_immediately=True))
        stream = await client.send_message(request)
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
        cancelled = await client.cancel_task(CancelRequest(id=first_task))
        require(task_id(cancelled) == first_task, "1.0 CancelTask addresses the live streamed task")
        current = await client.get_task(GetRequest(id=first_task))
        require(task_id(current) == first_task, "1.0 GetTask returns the streamed task")
        require("cancel" in task_state(current), "1.0 task status reflects Core cancellation")


if SDK_VERSION == "0.3.26":
    sdk_03()
elif SDK_VERSION == "1.0.3":
    sdk_10()
else:
    raise SystemExit(f"unsupported pinned SDK: {SDK_VERSION}")
