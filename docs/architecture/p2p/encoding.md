# Encoding

The codec for client -> server transport is a wrapper around the [Witnet network protocol][network protocol] which includes an
extra `u16` to indicate the message length. This limits the size of a message to 64KiB.

| Field  | Type | Description |
|--------|:----:|-------------|
| length  | u16  | message length |
| data    | [u8; length] | message data |

[network protocol]: ../../../protocol/network