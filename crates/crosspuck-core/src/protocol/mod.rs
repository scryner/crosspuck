pub mod codec;
pub mod frame;
pub mod payload;
pub mod queue;
pub mod status;

pub use codec::{ProtocolError, WireDecode, WireEncode, WirePayload};
pub use frame::{
    read_frame, write_frame, Channel, Frame, FrameError, FrameHeader, FrameIoError, MessageType,
    CONTROL_ADDR, CONTROL_PAYLOAD_LIMIT, FRAME_HEADER_LEN, INPUT_ADDR, INPUT_PAYLOAD_LIMIT, MAGIC,
    PROTOCOL_VERSION,
};
pub use payload::{
    CollectionDescriptor, CollectionRole, FeatureResult, GetFeature, Hello, HelloOk,
    IdentityPayload, InputAttach, InputAttachOk, InputReport, IoctlCommand, IoctlResult,
    SetFeature, SetFeatureResult, SetOutput, SetOutputResult, WriteReport, WriteResult,
    DEFAULT_GUEST_CAPABILITIES, DEFAULT_INPUT_QUEUE_CAPACITY,
};
pub use queue::{InputQueueStats, InputReportQueue, QueuedInputReport};
pub use status::{InvalidStatusCode, StatusCode};
