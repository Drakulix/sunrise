use openssl::{
    asn1::Asn1Time,
    bn::BigNum,
    error::ErrorStack,
    hash::MessageDigest,
    md::MdRef,
    md_ctx::MdCtx,
    pkey::{PKey, PKeyRef, Private},
    rand::rand_bytes,
    rsa::Rsa,
    sha::Sha256,
    symm::{Cipher, Crypter, Mode},
    x509::{X509Builder, X509NameBuilder, X509Ref, X509},
};

pub fn gen_aes_key(salt: &[u8], pin: &str) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(salt);
    hash.update(pin.as_bytes());
    let mut key = Vec::from(hash.finish());
    key.truncate(16);
    key
}

pub fn gen_creds() -> Result<(X509, PKey<Private>), ErrorStack> {
    let mut x509 = X509Builder::new().unwrap();
    let rsa = Rsa::generate(2048).unwrap();
    let pkey = PKey::from_rsa(rsa).unwrap();

    x509.set_version(2)?;
    x509.set_serial_number(&BigNum::from_u32(0).unwrap().to_asn1_integer().unwrap())?;
    x509.set_not_before(Asn1Time::days_from_now(0)?.as_ref())?;
    x509.set_not_after(Asn1Time::days_from_now(20 * 365)?.as_ref())?;
    x509.set_pubkey(&pkey)?;

    let mut name = X509NameBuilder::new()?;
    name.append_entry_by_text("CN", "Sunrise Server")?;
    let name = name.build();
    x509.set_subject_name(&name)?;
    x509.set_issuer_name(&name)?;

    x509.sign(&pkey, MessageDigest::sha256())?;

    Ok((x509.build(), pkey))
}

pub fn aes_decrypt_ecb<A: AsRef<[u8]>>(
    payload: A,
    key: &[u8],
    padding: bool,
) -> Result<Vec<u8>, ErrorStack> {
    aes_ecb(payload, key, Mode::Decrypt, padding)
}

pub fn aes_encrypt_ecb<A: AsRef<[u8]>>(
    payload: A,
    key: &[u8],
    padding: bool,
) -> Result<Vec<u8>, ErrorStack> {
    aes_ecb(payload, key, Mode::Encrypt, padding)
}

fn aes_ecb<A: AsRef<[u8]>>(
    payload: A,
    key: &[u8],
    mode: Mode,
    padding: bool,
) -> Result<Vec<u8>, ErrorStack> {
    let cipher = Cipher::aes_128_ecb();
    let mut iv = vec![0; cipher.block_size()];
    rand_bytes(&mut iv)?;

    let mut crypter = Crypter::new(cipher, mode, key, Some(&iv))?;
    crypter.pad(padding);

    let mut plaintext = vec![0; payload.as_ref().len() + cipher.block_size()];
    let mut len = 0;
    len += crypter.update(payload.as_ref(), &mut plaintext)?;
    len += crypter.finalize(&mut plaintext)?;
    plaintext.truncate(len);
    Ok(plaintext)
}

pub fn sign<A: AsRef<[u8]>>(
    pkey: &PKeyRef<Private>,
    payload: A,
    alg: &'static MdRef,
) -> Result<Vec<u8>, ErrorStack> {
    let mut ctx = MdCtx::new()?;
    ctx.digest_sign_init(Some(alg), pkey)?;

    ctx.digest_sign_update(payload.as_ref())?;

    let mut digest = Vec::new();
    ctx.digest_sign_final_to_vec(&mut digest)?;

    Ok(digest)
}

pub fn verify<A: AsRef<[u8]>>(
    cert: &X509Ref,
    payload: A,
    signature: &[u8],
    alg: &'static MdRef,
) -> Result<bool, ErrorStack> {
    let mut ctx = MdCtx::new()?;
    let pkey: PKey<Private> = unsafe { std::mem::transmute(cert.public_key()?) };

    ctx.digest_verify_init(Some(alg), &pkey)?;
    ctx.digest_verify_update(payload.as_ref())?;
    ctx.digest_verify_final(signature)
}
