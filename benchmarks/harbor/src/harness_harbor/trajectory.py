import base64
import hashlib
import json
from collections.abc import Iterable
from pathlib import Path
from typing import Any

from harbor.models.trajectories import (
    Agent,
    ContentPart,
    FinalMetrics,
    ImageSource,
    Metrics,
    Observation,
    ObservationResult,
    Step,
    ToolCall,
    Trajectory,
)

RUNNING = (
    "Tool call is still running. Its result arrives in a later turn: "
    "continue with independent work, or end your turn to wait for it."
)
TERMINAL = {"completed", "failed", "canceled"}


def bash_result(data: dict[str, Any]) -> str:
    status = data["Status"]
    operations = {op["ID"]: op for op in data.get("Operations", [])}
    waiting = status.get("WaitingFor") or []
    if status.get("Error"):
        if waiting or operations:
            raise ValueError("Bash call has both a validation error and operations")
        return "Error: " + status["Error"]
    if len(waiting) != 1:
        raise ValueError(f"Bash call has {len(waiting)} operations, want 1")
    op = operations[waiting[0]]
    if op["Type"] != "shell":
        raise ValueError(f"Unsupported operation type: {op['Type']}")
    state = op["State"]
    if op["Status"] in {"ready", "awaiting", "canceling"}:
        return RUNNING
    if op["Status"] not in TERMINAL:
        raise ValueError(f"Unsupported operation status: {op['Status']}")

    error = state.get("TerminalError") or ""
    if not error and op["Status"] in {"failed", "canceled"}:
        error = "shell operation " + op["Status"]
    parts = []
    value = state.get("Result")
    if value is not None:
        stdout, stderr = value["Out"], value["Err"]
        if not isinstance(stdout, str) or not isinstance(stderr, str):
            raise ValueError("Expected current runner text output")
        if stdout:
            parts.append(stdout)
        if stderr:
            parts.append("Stderr:\n" + stderr)
        if value["ExitCode"] != 0:
            parts.append(f"Exit code: {value['ExitCode']}")
    elif op["Status"] == "completed":
        raise ValueError("Completed shell operation has no result")
    else:
        if state.get("OutPath"):
            parts.append("Stdout capture: " + state["OutPath"])
        if state.get("ErrPath"):
            parts.append("Stderr capture: " + state["ErrPath"])
    if error:
        parts.append("Error: " + error)
    return "\n".join(parts) if parts else "(no output)"


def view_image_result(
    data: dict[str, Any], output_dir: Path | None
) -> str | list[ContentPart]:
    status = data["Status"]
    operations = {op["ID"]: op for op in data.get("Operations", [])}
    waiting = status.get("WaitingFor") or []
    if status.get("Error"):
        if waiting or operations:
            raise ValueError(
                "ViewImage call has both a validation error and operations"
            )
        return "Error: " + status["Error"]
    if len(waiting) != 1:
        raise ValueError(f"ViewImage call has {len(waiting)} operations, want 1")
    op = operations[waiting[0]]
    if op["Type"] != "view_image":
        raise ValueError(f"Unsupported operation type: {op['Type']}")
    if op["Status"] in {"ready", "awaiting", "canceling"}:
        return RUNNING
    if op["Status"] not in TERMINAL:
        raise ValueError(f"Unsupported operation status: {op['Status']}")

    failed = op["Status"] != "completed"
    value = op["State"].get("Result") or {}
    details = []
    if failed:
        error = value.get("Error") or "view-image operation " + op["Status"]
        details.append("Error: " + error)
    elif (
        not value.get("Content")
        or value.get("EncodedMIMEType") not in {"image/jpeg", "image/png"}
        or not 0 < value.get("ScaleRatio", 0) <= 1
        or value.get("Error")
    ):
        raise ValueError("Completed ViewImage operation has an invalid image result")

    original = value.get("OriginalMIMEType")
    if original and (failed or original != value.get("EncodedMIMEType")):
        details.append("original MIME type: " + original)
    width, height = value.get("OriginalWidth", 0), value.get("OriginalHeight", 0)
    ratio = value.get("ScaleRatio", 1)
    if width > 0 and height > 0 and (failed or ratio < 1):
        details.append(f"original dimensions: {width}x{height}")
    if failed:
        return "; ".join(details)
    if ratio < 1:
        details.append(
            f"multiply coordinates by {1 / ratio:.2f} to approximate original"
        )

    if output_dir is None:
        raise ValueError("ViewImage observations require an output directory")
    image = base64.b64decode(value["Content"], validate=True)
    extension = "jpg" if value["EncodedMIMEType"] == "image/jpeg" else "png"
    relative = Path("images") / f"{hashlib.sha256(image).hexdigest()}.{extension}"
    path = output_dir / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(image)
    content = [
        ContentPart(
            type="image",
            source=ImageSource(
                media_type=value["EncodedMIMEType"], path=relative.as_posix()
            ),
        )
    ]
    if details:
        content.append(ContentPart(type="text", text="; ".join(details)))
    return content


def convert(
    lines: Iterable[str],
    agent: Agent,
    session_id: str,
    *,
    output_dir: Path | None = None,
) -> Trajectory:
    steps: list[Step] = []
    calls: dict[str, tuple[Step, str]] = {}
    pending_observations: list[ObservationResult] = []
    errors: list[str] = []
    totals = dict(prompt=0, completion=0, cached=0, reasoning=0, cache_write=0)
    # Compaction turns summarize the conversation; their steps are marked.
    compactions: set[str] = set()
    previous_sequence = 0
    for line_number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValueError(f"Invalid runner JSONL at line {line_number}") from exc
        if item.get("type") == "error":
            errors.append(item["message"])
            continue
        sequence = item["Sequence"]
        if sequence <= previous_sequence:
            raise ValueError("Runner records must have increasing sequence numbers")
        previous_sequence = sequence
        kind, data = item["Kind"], item["Data"]
        timestamp = item["RecordedAt"]
        extra = {"sequence": sequence}
        if kind == "turn":
            for result in pending_observations:
                result.extra["available_before_turn"] = data["ID"]
            pending_observations.clear()
            if data.get("Type") == "compaction":
                compactions.add(data["ID"])
        elif kind == "input":
            if data["Kind"] != "external":
                continue
            if not isinstance(data["Payload"], str):
                raise ValueError("External runner input must be text")
            steps.append(
                Step(
                    step_id=len(steps) + 1,
                    source="user",
                    message=data["Payload"],
                    timestamp=timestamp,
                    extra=extra,
                )
            )
        elif kind == "model_response":
            response = data["Response"]
            compaction = data["TurnID"] in compactions
            texts, reasoning, tool_calls = [], [], []
            for output in response.get("Output", []):
                value = output["Data"]
                match output["Type"]:
                    case "message":
                        texts.append(value["Text"])
                    case "reasoning":
                        reasoning.extend(value.get("Summary", []))
                    case "tool_call" if compaction:
                        # The runner ignores calls in a summary.
                        continue
                    case "tool_call":
                        raw = value["Arguments"]
                        try:
                            arguments = json.loads(raw)
                        except json.JSONDecodeError:
                            arguments = {"raw_arguments": raw}
                        if not isinstance(arguments, dict):
                            arguments = {"raw_arguments": raw}
                        tool_calls.append(
                            ToolCall(
                                tool_call_id=value["CallID"],
                                function_name=value["Name"],
                                arguments=arguments,
                            )
                        )
                    case _:
                        raise ValueError(f"Unsupported model output: {output['Type']}")
            usage = response["Usage"]
            counts = {
                "prompt": usage["InputTokens"],
                "completion": usage["OutputTokens"],
                "cached": usage["CachedInputTokens"],
                "reasoning": usage["ReasoningTokens"],
                "cache_write": usage["CacheWriteInputTokens"],
            }
            for key, count in counts.items():
                totals[key] += count
            step = Step(
                step_id=len(steps) + 1,
                source="agent",
                timestamp=timestamp,
                message="\n\n".join(texts),
                reasoning_content="\n\n".join(reasoning) or None,
                model_name=agent.model_name,
                tool_calls=tool_calls or None,
                metrics=Metrics(
                    prompt_tokens=counts["prompt"],
                    completion_tokens=counts["completion"],
                    cached_tokens=counts["cached"],
                    extra={
                        "reasoning_tokens": counts["reasoning"],
                        "cache_write_tokens": counts["cache_write"],
                    },
                ),
                llm_call_count=1,
                extra={
                    **extra,
                    "turn_id": data["TurnID"],
                    "stop": response["Stop"],
                    "failure": response.get("Failure"),
                    **({"compaction": True} if compaction else {}),
                },
            )
            steps.append(step)
            for call in tool_calls:
                if call.tool_call_id in calls:
                    raise ValueError(f"Duplicate tool call: {call.tool_call_id}")
                calls[call.tool_call_id] = step, call.function_name
        elif kind == "tool_call_status":
            step, name = calls[data["CallID"]]
            if name == "Bash":
                content = bash_result(data)
            elif name == "ViewImage":
                content = view_image_result(data, output_dir)
            elif data["Status"].get("Error") and not data.get("Operations"):
                content = data["Status"]["Error"]
            else:
                raise ValueError(f"Unsupported tool result: {name}")
            pending_observations = [
                result
                for result in pending_observations
                if not (
                    result.source_call_id == data["CallID"]
                    and result.content == RUNNING
                )
            ]
            result = ObservationResult(
                source_call_id=data["CallID"],
                content=content,
                extra={**extra, "timestamp": timestamp},
            )
            if step.observation is None:
                step.observation = Observation(results=[])
            step.observation.results.append(result)
            pending_observations.append(result)
        else:
            raise ValueError(f"Unsupported runner record: {kind}")
    return Trajectory(
        schema_version="ATIF-v1.7",
        session_id=session_id,
        agent=agent,
        steps=steps,
        notes=(
            "Observations are attached to their originating tool calls. "
            "They may arrive asynchronously after later agent steps; use sequence, "
            "timestamp and available_before_turn metadata for ordering. "
            "This is a session audit, not a complete provider-request replay. "
            "Costs are unknown: runner usage contains tokens, not billed amounts."
        ),
        final_metrics=FinalMetrics(
            total_prompt_tokens=totals["prompt"],
            total_completion_tokens=totals["completion"],
            total_cached_tokens=totals["cached"],
            total_steps=len(steps),
            extra={
                "reasoning_tokens": totals["reasoning"],
                "cache_write_tokens": totals["cache_write"],
            },
        ),
        extra={"runner_errors": errors},
    )
