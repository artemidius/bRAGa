# SPDX-License-Identifier: Apache-2.0
"""The public result types: retrieval hits and the doctor report."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Mapping


@dataclass(frozen=True)
class Hit:
    """One retrieval hit, mirroring the JSON object ``braga_query`` emits."""

    chunk_id: str
    path: str
    line_start: int
    line_end: int
    content: str
    score: float

    @classmethod
    def from_json(cls, payload: Mapping[str, Any]) -> Hit:
        """Builds a hit from one element of the query's JSON array."""
        return cls(
            chunk_id=str(payload["chunk_id"]),
            path=str(payload["path"]),
            line_start=int(payload["line_start"]),
            line_end=int(payload["line_end"]),
            content=str(payload["content"]),
            score=float(payload["score"]),
        )


@dataclass(frozen=True)
class DoctorCheck:
    """One check in the doctor report, in the order the library ran it."""

    name: str
    passed: bool
    detail: str

    @classmethod
    def from_json(cls, payload: Mapping[str, Any]) -> DoctorCheck:
        """Builds a check from one element of the report's ``checks`` array."""
        return cls(
            name=str(payload["name"]),
            passed=bool(payload["passed"]),
            detail=str(payload["detail"]),
        )


@dataclass(frozen=True)
class DoctorReport:
    """The claims verifier's outcome: the checks and their conjunction."""

    ok: bool
    checks: list[DoctorCheck]

    @classmethod
    def from_json(cls, payload: Mapping[str, Any]) -> DoctorReport:
        """Builds a report from the JSON object ``braga_doctor`` emits."""
        return cls(
            ok=bool(payload["ok"]),
            checks=[DoctorCheck.from_json(check) for check in payload["checks"]],
        )
