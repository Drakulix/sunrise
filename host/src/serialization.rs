pub mod cert {
    use serde::{
        de::{self, Visitor},
        Deserializer, Serializer,
    };
    use std::fmt;

    use openssl::x509::X509;

    pub fn serialize<S>(cert: &X509, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ser.serialize_bytes(&cert.to_pem().unwrap())
    }
    pub fn deserialize<'de, D>(de: D) -> Result<X509, D::Error>
    where
        D: Deserializer<'de>,
    {
        de.deserialize_byte_buf(CertVisitor)
    }

    struct CertVisitor;

    impl<'de> Visitor<'de> for CertVisitor {
        type Value = X509;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a PEM encoded X509 certificate")
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            X509::from_pem(v).map_err(|_err| {
                de::Error::invalid_value(
                    std::str::from_utf8(v)
                        .map(de::Unexpected::Str)
                        .unwrap_or(de::Unexpected::Bytes(v)),
                    &"a PEM encoded X509 certificate",
                )
            })
        }

        fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            self.visit_bytes(&v)
        }
    }
}

pub mod key {
    use serde::{
        de::{Unexpected, Visitor},
        ser::Error,
        Deserializer, Serializer,
    };
    use std::fmt;

    use openssl::pkey::{PKey, Private};

    pub fn serialize<S>(key: &PKey<Private>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ser.serialize_bytes(
            &key.private_key_to_pem_pkcs8()
                .map_err(|err| S::Error::custom(err))?,
        )
    }
    pub fn deserialize<'de, D>(de: D) -> Result<PKey<Private>, D::Error>
    where
        D: Deserializer<'de>,
    {
        de.deserialize_bytes(KeyVisitor)
    }

    struct KeyVisitor;

    impl<'de> Visitor<'de> for KeyVisitor {
        type Value = PKey<Private>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a PEM encoded RSA Private Key")
        }

        fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            self.visit_bytes(&v)
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            PKey::private_key_from_pem(v).map_err(|_err| {
                E::invalid_value(
                    std::str::from_utf8(v)
                        .map(Unexpected::Str)
                        .unwrap_or(Unexpected::Bytes(v)),
                    &"a PEM encoded RSA Private Key",
                )
            })
        }
    }
}

pub fn get_default_interface() -> default_net::Interface {
    default_net::get_default_interface().unwrap_or_else(|_| {
        default_net::get_interfaces()
            .into_iter()
            .next()
            .expect("Failed to get network Interface")
    })
}
