"""SQLAlchemy ORM models for queryable security event storage."""

from agent_sec_cli.security_events.orm_base import Base
from agent_sec_cli.security_events.orm_store import register_orm_models
from sqlalchemy import Float, Index, Integer, Text
from sqlalchemy.orm import Mapped, mapped_column


class SecurityEventRecord(Base):
    """ORM mapping for the queryable security event index."""

    __tablename__ = "security_events"
    __table_args__ = (
        Index("idx_event_type", "event_type"),
        Index("idx_category_epoch", "category", "timestamp_epoch"),
        Index("idx_trace_id", "trace_id"),
        Index("idx_timestamp_epoch", "timestamp_epoch"),
        Index("idx_session_id_timestamp_epoch", "session_id", "timestamp_epoch"),
        Index("idx_run_id_timestamp_epoch", "run_id", "timestamp_epoch"),
        Index(
            "idx_session_run_timestamp_epoch",
            "session_id",
            "run_id",
            "timestamp_epoch",
        ),
    )
    __schema_columns__: dict[str, str] = {
        "run_id": "TEXT",
        "call_id": "TEXT",
        "tool_call_id": "TEXT",
    }

    event_id: Mapped[str] = mapped_column(Text, primary_key=True)
    event_type: Mapped[str] = mapped_column(Text, nullable=False)
    category: Mapped[str] = mapped_column(Text, nullable=False)
    result: Mapped[str] = mapped_column(
        Text, nullable=False, server_default="succeeded"
    )
    timestamp: Mapped[str] = mapped_column(Text, nullable=False)
    timestamp_epoch: Mapped[float] = mapped_column(Float, nullable=False)
    trace_id: Mapped[str] = mapped_column(Text, nullable=False, server_default="")
    pid: Mapped[int] = mapped_column(Integer, nullable=False)
    uid: Mapped[int] = mapped_column(Integer, nullable=False)
    session_id: Mapped[str | None] = mapped_column(Text, nullable=True)
    run_id: Mapped[str | None] = mapped_column(Text, nullable=True)
    call_id: Mapped[str | None] = mapped_column(Text, nullable=True)
    tool_call_id: Mapped[str | None] = mapped_column(Text, nullable=True)
    details: Mapped[str] = mapped_column(Text, nullable=False)


ORM_MODELS = (SecurityEventRecord,)
register_orm_models(ORM_MODELS)
