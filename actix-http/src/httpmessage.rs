use std::cell::{Ref, RefMut};
use std::str;

use encoding::all::UTF_8;
use encoding::label::encoding_from_whatwg_label;
use encoding::EncodingRef;
use http::header;
use mime::Mime;

use crate::cookie::Cookie;
use crate::error::{ContentTypeError, CookieParseError, ParseError};
use crate::extensions::Extensions;
use crate::header::{Header, HeaderMap};
use crate::payload::Payload;

struct Cookies(Vec<Cookie<'static>>);

/// Trait that implements general purpose operations on http messages
pub trait HttpMessage: Sized {
    /// Type of message payload stream
    type Stream;

    /// Read the message headers.
    fn headers(&self) -> &HeaderMap;

    /// Message payload stream
    fn take_payload(&mut self) -> Payload<Self::Stream>;

    /// Request's extensions container
    fn extensions(&self) -> Ref<Extensions>;

    /// Mutable reference to a the request's extensions container
    fn extensions_mut(&self) -> RefMut<Extensions>;

    #[doc(hidden)]
    /// Get a header
    fn get_header<H: Header>(&self) -> Option<H>
    where
        Self: Sized,
    {
        if self.headers().contains_key(H::name()) {
            H::parse(self).ok()
        } else {
            None
        }
    }

    /// Read the request content type. If request does not contain
    /// *Content-Type* header, empty str get returned.
    fn content_type(&self) -> &str {
        if let Some(content_type) = self.headers().get(header::CONTENT_TYPE) {
            if let Ok(content_type) = content_type.to_str() {
                return content_type.split(';').next().unwrap().trim();
            }
        }
        ""
    }

    /// Get content type encoding
    ///
    /// UTF-8 is used by default, If request charset is not set.
    fn encoding(&self) -> Result<EncodingRef, ContentTypeError> {
        if let Some(mime_type) = self.mime_type()? {
            if let Some(charset) = mime_type.get_param("charset") {
                if let Some(enc) = encoding_from_whatwg_label(charset.as_str()) {
                    Ok(enc)
                } else {
                    Err(ContentTypeError::UnknownEncoding)
                }
            } else {
                Ok(UTF_8)
            }
        } else {
            Ok(UTF_8)
        }
    }

    /// Convert the request content type to a known mime type.
    fn mime_type(&self) -> Result<Option<Mime>, ContentTypeError> {
        if let Some(content_type) = self.headers().get(header::CONTENT_TYPE) {
            if let Ok(content_type) = content_type.to_str() {
                return match content_type.parse() {
                    Ok(mt) => Ok(Some(mt)),
                    Err(_) => Err(ContentTypeError::ParseError),
                };
            } else {
                return Err(ContentTypeError::ParseError);
            }
        }
        Ok(None)
    }

    /// Check if request has chunked transfer encoding
    fn chunked(&self) -> Result<bool, ParseError> {
        if let Some(encodings) = self.headers().get(header::TRANSFER_ENCODING) {
            if let Ok(s) = encodings.to_str() {
                Ok(s.to_lowercase().contains("chunked"))
            } else {
                Err(ParseError::Header)
            }
        } else {
            Ok(false)
        }
    }

    /// Load request cookies.
    #[inline]
    fn cookies(&self) -> Result<Ref<Vec<Cookie<'static>>>, CookieParseError> {
        if self.extensions().get::<Cookies>().is_none() {
            let mut cookies = Vec::new();
            for hdr in self.headers().get_all(header::COOKIE) {
                let s =
                    str::from_utf8(hdr.as_bytes()).map_err(CookieParseError::from)?;
                for cookie_str in s.split(';').map(|s| s.trim()) {
                    if !cookie_str.is_empty() {
                        cookies.push(Cookie::parse_encoded(cookie_str)?.into_owned());
                    }
                }
            }
            self.extensions_mut().insert(Cookies(cookies));
        }
        Ok(Ref::map(self.extensions(), |ext| {
            &ext.get::<Cookies>().unwrap().0
        }))
    }

    /// Return request cookie.
    fn cookie(&self, name: &str) -> Option<Cookie<'static>> {
        if let Ok(cookies) = self.cookies() {
            for cookie in cookies.iter() {
                if cookie.name() == name {
                    return Some(cookie.to_owned());
                }
            }
        }
        None
    }
}

impl<'a, T> HttpMessage for &'a mut T
where
    T: HttpMessage,
{
    type Stream = T::Stream;

    fn headers(&self) -> &HeaderMap {
        (**self).headers()
    }

    /// Message payload stream
    fn take_payload(&mut self) -> Payload<Self::Stream> {
        (**self).take_payload()
    }

    /// Request's extensions container
    fn extensions(&self) -> Ref<Extensions> {
        (**self).extensions()
    }

    /// Mutable reference to a the request's extensions container
    fn extensions_mut(&self) -> RefMut<Extensions> {
        (**self).extensions_mut()
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use encoding::all::ISO_8859_2;
    use encoding::Encoding;
    use mime;

    use super::*;
    use crate::test::TestRequest;

    #[test]
    fn test_content_type() {
        let req = TestRequest::with_header("content-type", "text/plain").finish();
        assert_eq!(req.content_type(), "text/plain");
        let req =
            TestRequest::with_header("content-type", "application/json; charset=utf=8")
                .finish();
        assert_eq!(req.content_type(), "application/json");
        let req = TestRequest::default().finish();
        assert_eq!(req.content_type(), "");
    }

    #[test]
    fn test_mime_type() {
        let req = TestRequest::with_header("content-type", "application/json").finish();
        assert_eq!(req.mime_type().unwrap(), Some(mime::APPLICATION_JSON));
        let req = TestRequest::default().finish();
        assert_eq!(req.mime_type().unwrap(), None);
        let req =
            TestRequest::with_header("content-type", "application/json; charset=utf-8")
                .finish();
        let mt = req.mime_type().unwrap().unwrap();
        assert_eq!(mt.get_param(mime::CHARSET), Some(mime::UTF_8));
        assert_eq!(mt.type_(), mime::APPLICATION);
        assert_eq!(mt.subtype(), mime::JSON);
    }

    #[test]
    fn test_mime_type_error() {
        let req = TestRequest::with_header(
            "content-type",
            "applicationadfadsfasdflknadsfklnadsfjson",
        )
        .finish();
        assert_eq!(Err(ContentTypeError::ParseError), req.mime_type());
    }

    #[test]
    fn test_encoding() {
        let req = TestRequest::default().finish();
        assert_eq!(UTF_8.name(), req.encoding().unwrap().name());

        let req = TestRequest::with_header("content-type", "application/json").finish();
        assert_eq!(UTF_8.name(), req.encoding().unwrap().name());

        let req = TestRequest::with_header(
            "content-type",
            "application/json; charset=ISO-8859-2",
        )
        .finish();
        assert_eq!(ISO_8859_2.name(), req.encoding().unwrap().name());
    }

    #[test]
    fn test_encoding_error() {
        let req = TestRequest::with_header("content-type", "applicatjson").finish();
        assert_eq!(Some(ContentTypeError::ParseError), req.encoding().err());

        let req = TestRequest::with_header(
            "content-type",
            "application/json; charset=kkkttktk",
        )
        .finish();
        assert_eq!(
            Some(ContentTypeError::UnknownEncoding),
            req.encoding().err()
        );
    }

    #[test]
    fn test_chunked() {
        let req = TestRequest::default().finish();
        assert!(!req.chunked().unwrap());

        let req =
            TestRequest::with_header(header::TRANSFER_ENCODING, "chunked").finish();
        assert!(req.chunked().unwrap());

        let req = TestRequest::default()
            .header(
                header::TRANSFER_ENCODING,
                Bytes::from_static(b"some va\xadscc\xacas0xsdasdlue"),
            )
            .finish();
        assert!(req.chunked().is_err());
    }
}
