use std::net::SocketAddr;

use bytecodec::fixnum::{U32beDecoder, U32beEncoder};
use bytecodec::{
    ByteCount, Decode, DecodeExt, Encode, EncodeExt, Eos, SizedEncode, TryTaggedDecode,
};
use stun_codec::macros::track;
use stun_codec::net::{SocketAddrDecoder, SocketAddrEncoder, socket_addr_xor};
use stun_codec::rfc5389::attributes::{
    MappedAddress, Software, XorMappedAddress, XorMappedAddress2,
};
use stun_codec::rfc5389::methods::BINDING;
use stun_codec::rfc5780::attributes::{OtherAddress, ResponseOrigin};
use stun_codec::{
    AttributeType, Message, MessageClass, MessageDecoder, MessageEncoder, TransactionId,
    define_attribute_enums,
};

macro_rules! impl_decode {
    ($decoder:ty, $item:ident, $and_then:expr) => {
        impl Decode for $decoder {
            type Item = $item;

            fn decode(&mut self, buf: &[u8], eos: Eos) -> bytecodec::Result<usize> {
                track!(self.0.decode(buf, eos))
            }

            fn finish_decoding(&mut self) -> bytecodec::Result<Self::Item> {
                track!(self.0.finish_decoding()).and_then($and_then)
            }

            fn requiring_bytes(&self) -> ByteCount {
                self.0.requiring_bytes()
            }

            fn is_idle(&self) -> bool {
                self.0.is_idle()
            }
        }

        impl TryTaggedDecode for $decoder {
            type Tag = AttributeType;

            fn try_start_decoding(&mut self, attr_type: Self::Tag) -> bytecodec::Result<bool> {
                Ok(attr_type.as_u16() == $item::CODEPOINT)
            }
        }
    };
}

macro_rules! impl_encode {
    ($encoder:ty, $item:ty, $map_from:expr) => {
        impl Encode for $encoder {
            type Item = $item;

            fn encode(&mut self, buf: &mut [u8], eos: Eos) -> bytecodec::Result<usize> {
                track!(self.0.encode(buf, eos))
            }

            fn start_encoding(&mut self, item: Self::Item) -> bytecodec::Result<()> {
                track!(self.0.start_encoding($map_from(item)))
            }

            fn requiring_bytes(&self) -> ByteCount {
                self.0.requiring_bytes()
            }

            fn is_idle(&self) -> bool {
                self.0.is_idle()
            }
        }

        impl SizedEncode for $encoder {
            fn exact_requiring_bytes(&self) -> u64 {
                self.0.exact_requiring_bytes()
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChangedAddress(SocketAddr);

impl ChangedAddress {
    pub const CODEPOINT: u16 = 0x0005;

    pub fn new(addr: SocketAddr) -> Self {
        Self(addr)
    }

    pub fn address(&self) -> SocketAddr {
        self.0
    }
}

impl stun_codec::Attribute for ChangedAddress {
    type Decoder = ChangedAddressDecoder;
    type Encoder = ChangedAddressEncoder;

    fn get_type(&self) -> AttributeType {
        AttributeType::new(Self::CODEPOINT)
    }

    fn before_encode<A: stun_codec::Attribute>(
        &mut self,
        message: &Message<A>,
    ) -> bytecodec::Result<()> {
        self.0 = socket_addr_xor(self.0, message.transaction_id());
        Ok(())
    }

    fn after_decode<A: stun_codec::Attribute>(
        &mut self,
        message: &Message<A>,
    ) -> bytecodec::Result<()> {
        self.0 = socket_addr_xor(self.0, message.transaction_id());
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct ChangedAddressDecoder(SocketAddrDecoder);
impl_decode!(ChangedAddressDecoder, ChangedAddress, |item| Ok(
    ChangedAddress(item)
));

#[derive(Debug, Default)]
pub struct ChangedAddressEncoder(SocketAddrEncoder);
impl_encode!(ChangedAddressEncoder, ChangedAddress, |item: Self::Item| {
    item.0
});

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChangeRequest(bool, bool);

impl ChangeRequest {
    pub const CODEPOINT: u16 = 0x0003;

    pub fn new(ip: bool, port: bool) -> Self {
        Self(ip, port)
    }

    pub fn ip(&self) -> bool {
        self.0
    }

    pub fn port(&self) -> bool {
        self.1
    }
}

impl stun_codec::Attribute for ChangeRequest {
    type Decoder = ChangeRequestDecoder;
    type Encoder = ChangeRequestEncoder;

    fn get_type(&self) -> AttributeType {
        AttributeType::new(Self::CODEPOINT)
    }
}

#[derive(Debug, Default)]
pub struct ChangeRequestDecoder(U32beDecoder);
impl_decode!(ChangeRequestDecoder, ChangeRequest, |item| {
    Ok(ChangeRequest((item & 0x4) != 0, (item & 0x2) != 0))
});

#[derive(Debug, Default)]
pub struct ChangeRequestEncoder(U32beEncoder);
impl_encode!(ChangeRequestEncoder, ChangeRequest, |item: Self::Item| {
    let ip = item.0 as u8;
    let port = item.1 as u8;
    ((ip << 1 | port) << 1) as u32
});

define_attribute_enums!(
    Attribute,
    AttributeDecoder,
    AttributeEncoder,
    [
        Software,
        MappedAddress,
        XorMappedAddress,
        XorMappedAddress2,
        OtherAddress,
        ChangeRequest,
        ChangedAddress,
        ResponseOrigin
    ]
);

#[derive(Debug)]
pub struct BindingRequest {
    pub transaction_id: TransactionId,
    pub change_ip: bool,
    pub change_port: bool,
}

pub fn decode_binding_request(bytes: &[u8]) -> bytecodec::Result<Option<BindingRequest>> {
    let mut decoder = MessageDecoder::<Attribute>::new();
    let Ok(message) = decoder.decode_from_bytes(bytes)? else {
        return Ok(None);
    };
    if message.class() != MessageClass::Request || message.method() != BINDING {
        return Ok(None);
    }

    let mut change_ip = false;
    let mut change_port = false;
    for attribute in message.attributes() {
        if let Attribute::ChangeRequest(change) = attribute {
            change_ip = change.ip();
            change_port = change.port();
        }
    }
    Ok(Some(BindingRequest {
        transaction_id: message.transaction_id(),
        change_ip,
        change_port,
    }))
}

pub fn encode_binding_response(
    request: &BindingRequest,
    peer_addr: SocketAddr,
    response_origin: SocketAddr,
    other_addr: SocketAddr,
    software: &str,
) -> bytecodec::Result<Vec<u8>> {
    let mut response = Message::<Attribute>::new(
        MessageClass::SuccessResponse,
        BINDING,
        request.transaction_id,
    );
    response.add_attribute(Attribute::XorMappedAddress(XorMappedAddress::new(
        peer_addr,
    )));
    response.add_attribute(Attribute::ResponseOrigin(ResponseOrigin::new(
        response_origin,
    )));
    response.add_attribute(Attribute::OtherAddress(OtherAddress::new(other_addr)));
    response.add_attribute(Attribute::ChangedAddress(ChangedAddress::new(other_addr)));
    response.add_attribute(Attribute::Software(Software::new(software.to_owned())?));

    MessageEncoder::new()
        .encode_into_bytes(response)
        .map(|bytes| bytes.as_slice().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn easytier_transaction_id(value: u32) -> TransactionId {
        let mut bytes = [0_u8; 12];
        bytes[..4].copy_from_slice(&0xdead_beef_u32.to_be_bytes());
        bytes[8..].copy_from_slice(&value.to_le_bytes());
        TransactionId::new(bytes)
    }

    #[test]
    fn decodes_easytier_change_request() {
        let mut message =
            Message::<Attribute>::new(MessageClass::Request, BINDING, easytier_transaction_id(42));
        message.add_attribute(Attribute::ChangeRequest(ChangeRequest::new(true, true)));
        let bytes = MessageEncoder::new().encode_into_bytes(message).unwrap();
        let request = decode_binding_request(bytes.as_slice()).unwrap().unwrap();
        assert!(request.change_ip);
        assert!(request.change_port);
    }

    #[test]
    fn response_contains_addresses_easytier_reads() {
        let request = BindingRequest {
            transaction_id: easytier_transaction_id(7),
            change_ip: false,
            change_port: false,
        };
        let peer = "198.51.100.9:40000".parse().unwrap();
        let origin = "203.0.113.10:3478".parse().unwrap();
        let other = "203.0.113.11:3479".parse().unwrap();
        let bytes =
            encode_binding_response(&request, peer, origin, other, "easytier-stun").unwrap();

        let mut decoder = MessageDecoder::<Attribute>::new();
        let response = decoder
            .decode_from_bytes(&bytes)
            .unwrap()
            .expect("complete response");
        assert_eq!(response.class(), MessageClass::SuccessResponse);
        assert!(response.attributes().any(
            |attr| matches!(attr, Attribute::XorMappedAddress(value) if value.address() == peer)
        ));
        assert!(response.attributes().any(
            |attr| matches!(attr, Attribute::OtherAddress(value) if value.address() == other)
        ));
        assert!(response.attributes().any(
            |attr| matches!(attr, Attribute::ChangedAddress(value) if value.address() == other)
        ));
    }
}
