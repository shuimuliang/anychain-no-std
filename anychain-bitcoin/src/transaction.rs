use crate::address::BitcoinAddress;
use crate::amount::BitcoinAmount;
use crate::format::BitcoinFormat;
use crate::network::BitcoinNetwork;
use crate::public_key::BitcoinPublicKey;
use crate::witness_program::WitnessProgram;
use anychain_core::no_std::{io::Read, *};
use anychain_core::{Transaction, TransactionError, TransactionId, crypto::checksum as double_sha2};

use base58::FromBase58;
use bech32::{self, FromBase32};
use core::{fmt, str::FromStr};
use serde::Serialize;
pub use sha2::{Digest, Sha256};

/// Returns the variable length integer of the given value.
/// https://en.bitcoin.it/wiki/Protocol_documentation#Variable_length_integer
pub fn variable_length_integer(value: u64) -> Result<Vec<u8>, TransactionError> {
    match value {
        // bounded by u8::max_value()
        0..=252 => Ok(vec![value as u8]),
        // bounded by u16::max_value()
        253..=65535 => Ok([vec![0xfd], (value as u16).to_le_bytes().to_vec()].concat()),
        // bounded by u32::max_value()
        65536..=4294967295 => Ok([vec![0xfe], (value as u32).to_le_bytes().to_vec()].concat()),
        // bounded by u64::max_value()
        _ => Ok([vec![0xff], value.to_le_bytes().to_vec()].concat()),
    }
}

/// Decode the value of a variable length integer.
/// https://en.bitcoin.it/wiki/Protocol_documentation#Variable_length_integer
pub fn read_variable_length_integer<R: Read>(mut reader: R) -> Result<usize, TransactionError> {
    let mut flag = [0u8; 1];
    reader.read(&mut flag)?;

    match flag[0] {
        0..=252 => Ok(flag[0] as usize),
        0xfd => {
            let mut size = [0u8; 2];
            reader.read(&mut size)?;
            match u16::from_le_bytes(size) {
                s if s < 253 => Err(TransactionError::InvalidVariableSizeInteger(s as usize)),
                s => Ok(s as usize),
            }
        }
        0xfe => {
            let mut size = [0u8; 4];
            reader.read(&mut size)?;
            match u32::from_le_bytes(size) {
                s if s < 65536 => Err(TransactionError::InvalidVariableSizeInteger(s as usize)),
                s => Ok(s as usize),
            }
        }
        _ => {
            let mut size = [0u8; 8];
            reader.read(&mut size)?;
            match u64::from_le_bytes(size) {
                s if s < 4294967296 => {
                    Err(TransactionError::InvalidVariableSizeInteger(s as usize))
                }
                s => Ok(s as usize),
            }
        }
    }
}

pub struct BitcoinVector;

impl BitcoinVector {
    /// Read and output a vector with a variable length integer
    pub fn read<R: Read, E, F>(mut reader: R, func: F) -> Result<Vec<E>, TransactionError>
    where
        F: Fn(&mut R) -> Result<E, TransactionError>,
    {
        let count = read_variable_length_integer(&mut reader)?;
        (0..count).map(|_| func(&mut reader)).collect()
    }

    /// Read and output a vector with a variable length integer and the integer itself
    pub fn read_witness<R: Read, E, F>(
        mut reader: R,
        func: F,
    ) -> Result<(usize, Result<Vec<E>, TransactionError>), TransactionError>
    where
        F: Fn(&mut R) -> Result<E, TransactionError>,
    {
        let count = read_variable_length_integer(&mut reader)?;
        Ok((count, Self::read(reader, func)))
    }
}

/// Generate the script_pub_key of a corresponding address
pub fn create_script_pub_key<N: BitcoinNetwork>(
    address: &BitcoinAddress<N>,
) -> Result<Vec<u8>, TransactionError> {
    match address.format() {
        BitcoinFormat::P2PKH => {
            let bytes = &address.to_string().from_base58()?;
            
            // Trim the prefix (1st byte) and the checksum (last 4 bytes)
            let pub_key_hash = bytes[1..(bytes.len() - 4)].to_vec();

            let mut script = vec![];
            script.push(Opcode::OP_DUP as u8);
            script.push(Opcode::OP_HASH160 as u8);
            script.extend(variable_length_integer(pub_key_hash.len() as u64)?);
            script.extend(pub_key_hash);
            script.push(Opcode::OP_EQUALVERIFY as u8);
            script.push(Opcode::OP_CHECKSIG as u8);
            Ok(script)
        }
        BitcoinFormat::P2WSH => {
            let (_hrp, data, _variant) = bech32::decode(&address.to_string())?;
            let (v, script) = data.split_at(1);
            let script = Vec::from_base32(script)?;
            let mut script_bytes = vec![v[0].to_u8(), script.len() as u8];
            script_bytes.extend(script);
            Ok(script_bytes)
        }
        BitcoinFormat::P2SH_P2WPKH => {
            let script_bytes = &address.to_string().from_base58()?;
            let script_hash = script_bytes[1..(script_bytes.len() - 4)].to_vec();

            let mut script = vec![];
            script.push(Opcode::OP_HASH160 as u8);
            script.extend(variable_length_integer(script_hash.len() as u64)?);
            script.extend(script_hash);
            script.push(Opcode::OP_EQUAL as u8);
            Ok(script)
        }
        BitcoinFormat::Bech32 => {
            let (_, data, _) = bech32::decode(&address.to_string())?;
            let (v, program) = data.split_at(1);
            let program = Vec::from_base32(program)?;
            let mut program_bytes = vec![v[0].to_u8(), program.len() as u8];
            program_bytes.extend(program);

            Ok(WitnessProgram::new(&program_bytes)?.to_scriptpubkey())
        }
    }
}

/// Construct and return the OP_RETURN script for the data
/// output of a tx that spends 'amount' basic units of omni
/// layer asset as indicated by 'property_id'.
pub fn create_script_op_return(property_id: u32, amount: i64) -> Result<Vec<u8>, TransactionError> {
    let mut script = vec![];

    let msg_type: u16 = 0;
    let msg_version: u16 = 0;

    script.push(Opcode::OP_RETURN as u8);
    script.push(Opcode::OP_PUSHBYTES_20 as u8);
    script.push(b'o');
    script.push(b'm');
    script.push(b'n');
    script.push(b'i');
    script.append(&mut msg_version.to_be_bytes().to_vec());
    script.append(&mut msg_type.to_be_bytes().to_vec());
    script.append(&mut property_id.to_be_bytes().to_vec());
    script.append(&mut amount.to_be_bytes().to_vec());

    Ok(script)
}

/// Represents a Bitcoin signature hash
/// https://en.bitcoin.it/wiki/OP_CHECKSIG
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[allow(non_camel_case_types)]
pub enum SignatureHash {
    /// Signs all inputs and outputs.
    SIGHASH_ALL = 0x01,

    /// Signs all inputs and none of the outputs.
    /// (e.g. "blank check" transaction, where any address can redeem the output)
    SIGHASH_NONE = 0x02,

    /// Signs all inputs and one corresponding output per input.
    /// (e.g. signing vin 0 will result in signing vout 0)
    SIGHASH_SINGLE = 0x03,

    SIGHASH_ALL_SIGHASH_FORKID = 0x41,
    SIGHASH_NONE_SIGHASH_FORKID = 0x42,
    SIGHASH_SINGLE_SIGHASH_FORKID = 0x43,

    /// Signs only one input and all outputs.
    /// Allows anyone to add or remove other inputs, forbids changing any outputs.
    /// (e.g. "crowdfunding" transaction, where the output is the "goal" address)
    SIGHASH_ALL_SIGHASH_ANYONECANPAY = 0x81,

    /// Signs only one input and none of the outputs.
    /// Allows anyone to add or remove other inputs or any outputs.
    /// (e.g. "dust collector" transaction, where "dust" can be aggregated and spent together)
    SIGHASH_NONE_SIGHASH_ANYONECANPAY = 0x82,

    /// Signs only one input and one corresponding output per input.
    /// Allows anyone to add or remove other inputs.
    SIGHASH_SINGLE_SIGHASH_ANYONECANPAY = 0x83,

    SIGHASH_ALL_SIGHASH_FORKID_SIGHASH_ANYONECANPAY = 0xc1,
    SIGHASH_NONE_SIGHASH_FORKID_SIGHASH_ANYONECANPAY = 0xc2,
    SIGHASH_SINGLE_SIGHASH_FORKID_SIGHASH_ANYONECANPAY = 0xc3,
}

impl fmt::Display for SignatureHash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SignatureHash::SIGHASH_ALL => write!(f, "SIGHASH_ALL"),
            SignatureHash::SIGHASH_NONE => write!(f, "SIGHASH_NONE"),
            SignatureHash::SIGHASH_SINGLE => write!(f, "SIGHASH_SINGLE"),
            SignatureHash::SIGHASH_ALL_SIGHASH_FORKID => {
                write!(f, "SIGHASH_ALL | SIGHASH_FORKID")
            }
            SignatureHash::SIGHASH_NONE_SIGHASH_FORKID => {
                write!(f, "SIGHASH_NONE | SIGHASH_FORKID")
            }
            SignatureHash::SIGHASH_SINGLE_SIGHASH_FORKID => {
                write!(f, "SIGHASH_SINGLE | SIGHASH_FORKID")
            }
            SignatureHash::SIGHASH_ALL_SIGHASH_ANYONECANPAY => {
                write!(f, "SIGHASH_ALL | SIGHASH_ANYONECANPAY")
            }
            SignatureHash::SIGHASH_NONE_SIGHASH_ANYONECANPAY => {
                write!(f, "SIGHASH_NONE | SIGHASH_ANYONECANPAY")
            }
            SignatureHash::SIGHASH_SINGLE_SIGHASH_ANYONECANPAY => {
                write!(f, "SIGHASH_SINGLE | SIGHASH_ANYONECANPAY")
            }
            SignatureHash::SIGHASH_ALL_SIGHASH_FORKID_SIGHASH_ANYONECANPAY => {
                write!(f, "SIGHASH_ALL | SIGHASH_FORKID | SIGHASH_ANYONECANPAY")
            }
            SignatureHash::SIGHASH_NONE_SIGHASH_FORKID_SIGHASH_ANYONECANPAY => {
                write!(f, "SIGHASH_NONE | SIGHASH_FORKID | SIGHASH_ANYONECANPAY")
            }
            SignatureHash::SIGHASH_SINGLE_SIGHASH_FORKID_SIGHASH_ANYONECANPAY => {
                write!(f, "SIGHASH_SINGLE | SIGHASH_FORKID | SIGHASH_ANYONECANPAY")
            }
        }
    }
}

impl SignatureHash {
    pub fn from_byte(byte: &u8) -> Self {
        match byte {
            0x01 => SignatureHash::SIGHASH_ALL,
            0x02 => SignatureHash::SIGHASH_NONE,
            0x03 => SignatureHash::SIGHASH_SINGLE,
            0x41 => SignatureHash::SIGHASH_ALL_SIGHASH_FORKID,
            0x42 => SignatureHash::SIGHASH_NONE_SIGHASH_FORKID,
            0x43 => SignatureHash::SIGHASH_SINGLE_SIGHASH_FORKID,
            0x81 => SignatureHash::SIGHASH_ALL_SIGHASH_ANYONECANPAY,
            0x82 => SignatureHash::SIGHASH_NONE_SIGHASH_ANYONECANPAY,
            0x83 => SignatureHash::SIGHASH_SINGLE_SIGHASH_ANYONECANPAY,
            0xc1 => SignatureHash::SIGHASH_ALL_SIGHASH_FORKID_SIGHASH_ANYONECANPAY,
            0xc2 => SignatureHash::SIGHASH_NONE_SIGHASH_FORKID_SIGHASH_ANYONECANPAY,
            0xc3 => SignatureHash::SIGHASH_SINGLE_SIGHASH_FORKID_SIGHASH_ANYONECANPAY,
            _ => panic!("Unrecognized signature hash"),
        }
    }
}

/// Represents the commonly used script opcodes
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[allow(non_camel_case_types)]
pub enum Opcode {
    OP_DUP = 0x76,
    OP_HASH160 = 0xa9,
    OP_CHECKSIG = 0xac,
    OP_EQUAL = 0x87,
    OP_EQUALVERIFY = 0x88,
    OP_RETURN = 0x6a,
    OP_PUSHBYTES_20 = 0x14,
}

impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Opcode::OP_DUP => write!(f, "OP_DUP"),
            Opcode::OP_HASH160 => write!(f, "OP_HASH160"),
            Opcode::OP_CHECKSIG => write!(f, "OP_CHECKSIG"),
            Opcode::OP_EQUAL => write!(f, "OP_EQUAL"),
            Opcode::OP_EQUALVERIFY => write!(f, "OP_EQUALVERIFY"),
            Opcode::OP_RETURN => write!(f, "OP_RETURN"),
            Opcode::OP_PUSHBYTES_20 => write!(f, "OP_PUSHBYTES_20"),
        }
    }
}

/// Represents a Bitcoin transaction outpoint
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Outpoint<N: BitcoinNetwork> {
    /// The previous transaction hash (32 bytes) (uses reversed hash order from Bitcoin RPC)
    pub reverse_transaction_id: Vec<u8>,
    /// The index of the transaction input (4 bytes)
    pub index: u32,
    /// The amount associated with this input (used for SegWit transaction signatures)
    pub amount: Option<BitcoinAmount>,
    /// The script public key associated with spending this input
    pub script_pub_key: Option<Vec<u8>>,
    /// An optional redeem script (for SegWit transactions)
    pub redeem_script: Option<Vec<u8>>,
    /// The address of the outpoint
    pub address: Option<BitcoinAddress<N>>,
}

impl<N: BitcoinNetwork> Outpoint<N> {
    /// Returns a new Bitcoin transaction outpoint
    pub fn new(
        reverse_transaction_id: Vec<u8>,
        index: u32,
        address: Option<BitcoinAddress<N>>,
        amount: Option<BitcoinAmount>,
        redeem_script: Option<Vec<u8>>,
        script_pub_key: Option<Vec<u8>>,
    ) -> Result<Self, TransactionError> {
        let (script_pub_key, redeem_script) = match address.clone() {
            Some(address) => {
                let script_pub_key =
                    script_pub_key.unwrap_or(create_script_pub_key::<N>(&address)?);
                let redeem_script = match address.format() {
                    BitcoinFormat::P2PKH => match redeem_script {
                        Some(_) => return Err(TransactionError::InvalidInputs("P2PKH".into())),
                        None => match script_pub_key[0] != Opcode::OP_DUP as u8
                            && script_pub_key[1] != Opcode::OP_HASH160 as u8
                            && script_pub_key[script_pub_key.len() - 1] != Opcode::OP_CHECKSIG as u8
                        {
                            true => {
                                return Err(TransactionError::InvalidScriptPubKey("P2PKH".into()))
                            }
                            false => None,
                        },
                    },
                    BitcoinFormat::P2WSH => match redeem_script {
                        Some(redeem_script) => match script_pub_key[0] != 0x00_u8
                            && script_pub_key[1] != 0x20_u8
                            && script_pub_key.len() != 34 // zero [32-byte sha256(witness script)]
                        {
                            true => return Err(TransactionError::InvalidScriptPubKey("P2WSH".into())),
                            false => Some(redeem_script),
                        },
                        None => return Err(TransactionError::InvalidInputs("P2WSH".into())),
                    },
                    BitcoinFormat::P2SH_P2WPKH => match redeem_script {
                        Some(redeem_script) => match script_pub_key[0] != Opcode::OP_HASH160 as u8
                            && script_pub_key[script_pub_key.len() - 1] != Opcode::OP_EQUAL as u8
                        {
                            true => {
                                return Err(TransactionError::InvalidScriptPubKey(
                                    "P2SH_P2WPKH".into(),
                                ))
                            }
                            false => Some(redeem_script),
                        },
                        None => return Err(TransactionError::InvalidInputs("P2SH_P2WPKH".into())),
                    },
                    BitcoinFormat::Bech32 => match redeem_script.is_some() {
                        true => return Err(TransactionError::InvalidInputs("Bech32".into())),
                        false => None,
                    },
                };

                (Some(script_pub_key), redeem_script)
            }
            None => (None, None),
        };

        Ok(Self {
            reverse_transaction_id,
            index,
            amount,
            redeem_script,
            script_pub_key,
            address,
        })
    }
}

/// Represents a Bitcoin transaction input
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BitcoinTransactionInput<N: BitcoinNetwork> {
    /// The outpoint (36 bytes)
    pub outpoint: Outpoint<N>,
    /// The transaction input script (variable size)
    pub script_sig: Vec<u8>,
    /// The sequence number (4 bytes) (0xFFFFFFFF unless lock > 0)
    /// Also used in replace-by-fee (BIP 125)
    pub sequence: Vec<u8>,
    /// The signature hash (4 bytes) (used in signing raw transaction only)
    pub sighash_code: SignatureHash,
    /// The witnesses in a SegWit transaction
    pub witnesses: Vec<Vec<u8>>,
    /// If true, the input has been signed
    pub is_signed: bool,
    /// Provide more flexibility for multiple signatures (for P2WSH)
    pub additional_witness: Option<(Vec<u8>, bool)>,
    /// Option for additional witness stack script args
    pub witness_script_data: Option<Vec<u8>>,
}

impl<N: BitcoinNetwork> BitcoinTransactionInput<N> {
    const DEFAULT_SEQUENCE: [u8; 4] = [0xff, 0xff, 0xff, 0xff];

    /// Returns a new Bitcoin transaction input without the script (unlocking).
    pub fn new(
        transaction_id: Vec<u8>,
        index: u32,
        address: Option<BitcoinAddress<N>>,
        amount: Option<BitcoinAmount>,
        redeem_script: Option<Vec<u8>>,
        script_pub_key: Option<Vec<u8>>,
    ) -> Result<Self, TransactionError> {
        if transaction_id.len() != 32 {
            return Err(TransactionError::InvalidTransactionId(transaction_id.len()));
        }

        // Byte-wise reverse of computed SHA-256 hash values
        // https://bitcoin.org/en/developer-reference#hash-byte-order
        let mut reverse_transaction_id = transaction_id;
        reverse_transaction_id.reverse();

        let outpoint = Outpoint::<N>::new(
            reverse_transaction_id,
            index,
            address,
            amount,
            redeem_script,
            script_pub_key,
        )?;

        Ok(Self {
            outpoint,
            script_sig: vec![],
            sequence: BitcoinTransactionInput::<N>::DEFAULT_SEQUENCE.to_vec(),
            sighash_code: SignatureHash::SIGHASH_ALL,
            witnesses: vec![],
            is_signed: false,
            additional_witness: None,
            witness_script_data: None,
        })
    }

    pub fn set_sequence(&mut self, sequence: Vec<u8>) {
        self.sequence = sequence;
    }

    pub fn set_sighash(&mut self, sighash: SignatureHash) {
        self.sighash_code = sighash;
    }

    /// Read and output a Bitcoin transaction input
    pub fn read<R: Read>(mut reader: &mut R) -> Result<Self, TransactionError> {
        let mut transaction_hash = [0u8; 32];
        let mut vin = [0u8; 4];
        let mut sequence = [0u8; 4];

        reader.read(&mut transaction_hash)?;
        reader.read(&mut vin)?;

        let outpoint = Outpoint::<N>::new(
            transaction_hash.to_vec(),
            u32::from_le_bytes(vin),
            None,
            None,
            None,
            None,
        )?;

        let script_sig: Vec<u8> = BitcoinVector::read(
            &mut reader,
            |s| {
                let mut byte = [0u8; 1];
                s.read(&mut byte)?;
                Ok(byte[0])
            }
        )?;

        reader.read(&mut sequence)?;

        let script_sig_len = read_variable_length_integer(&script_sig[..])?;
        
        let sighash_code = SignatureHash::from_byte(
            &match script_sig_len {
                0 => 0x01,
                length => script_sig[length],
            }
        );

        Ok(Self {
            outpoint,
            script_sig: script_sig.to_vec(),
            sequence: sequence.to_vec(),
            sighash_code,
            witnesses: vec![],
            is_signed: !script_sig.is_empty(),
            additional_witness: None,
            witness_script_data: None,
        })
    }

    /// Returns the serialized transaction input.
    pub fn serialize(&self, raw: bool) -> Result<Vec<u8>, TransactionError> {
        let mut input = vec![];
        input.extend(&self.outpoint.reverse_transaction_id);
        input.extend(&self.outpoint.index.to_le_bytes());

        match raw {
            true => input.extend(vec![0x00]),
            false => match self.script_sig.len() {
                0 => match &self.outpoint.address {
                    Some(address) => match address.format() {
                        BitcoinFormat::Bech32 => input.extend(vec![0x00]),
                        BitcoinFormat::P2WSH => input.extend(vec![0x00]),
                        _ => {
                            let script_pub_key = match &self.outpoint.script_pub_key {
                                Some(script) => script,
                                None => {
                                    return Err(TransactionError::MissingOutpointScriptPublicKey)
                                }
                            };
                            input.extend(variable_length_integer(script_pub_key.len() as u64)?);
                            input.extend(script_pub_key);
                        }
                    },
                    None => input.extend(vec![0x00]),
                },
                _ => {
                    input.extend(variable_length_integer(self.script_sig.len() as u64)?);
                    input.extend(&self.script_sig);
                }
            },
        };

        input.extend(&self.sequence);
        Ok(input)
    }
}

/// Represents a Bitcoin transaction output
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BitcoinTransactionOutput {
    /// The amount (in Satoshi)
    pub amount: BitcoinAmount,
    /// The public key script
    pub script_pub_key: Vec<u8>,
}

impl BitcoinTransactionOutput {
    /// Returns a Bitcoin transaction output.
    pub fn new<N: BitcoinNetwork>(
        address: &BitcoinAddress<N>,
        amount: BitcoinAmount,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            amount,
            script_pub_key: create_script_pub_key::<N>(address)?,
        })
    }

    /// Returns the data output for a tx that spends 'amount' basic
    /// units of omni-layer asset as indicated by 'property_id'.
    pub fn omni_data_output(
        property_id: u32,
        amount: BitcoinAmount,
    ) -> Result<Self, TransactionError> {
        let data_output = BitcoinTransactionOutput {
            amount: BitcoinAmount(0),
            script_pub_key: create_script_op_return(property_id, amount.0)?,
        };

        Ok(data_output)
    }

    /// Read and output a Bitcoin transaction output
    pub fn read<R: Read>(mut reader: &mut R) -> Result<Self, TransactionError> {
        let mut amount = [0u8; 8];
        reader.read(&mut amount)?;

        let script_pub_key: Vec<u8> = BitcoinVector::read(
            &mut reader,
            |s| {
                let mut byte = [0u8; 1];
                s.read(&mut byte)?;
                Ok(byte[0])
            }
        )?;

        Ok(Self {
            amount: BitcoinAmount::from_satoshi(u64::from_le_bytes(amount) as i64)?,
            script_pub_key,
        })
    }

    /// Returns the serialized transaction output.
    pub fn serialize(&self) -> Result<Vec<u8>, TransactionError> {
        let mut output = vec![];
        output.extend(&self.amount.0.to_le_bytes());
        output.extend(variable_length_integer(self.script_pub_key.len() as u64)?);
        output.extend(&self.script_pub_key);
        Ok(output)
    }
}

/// Represents an Bitcoin transaction id and witness transaction id
/// https://github.com/bitcoin/bips/blob/master/bip-0141.mediawiki#transaction-id
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BitcoinTransactionId {
    txid: Vec<u8>,
    wtxid: Vec<u8>,
}

impl TransactionId for BitcoinTransactionId {}

impl fmt::Display for BitcoinTransactionId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", &hex::encode(&self.txid))
    }
}

/// Represents the Bitcoin transaction parameters
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BitcoinTransactionParameters<N: BitcoinNetwork> {
    /// The version number (4 bytes)
    pub version: u32,
    /// The transaction inputs
    pub inputs: Vec<BitcoinTransactionInput<N>>,
    /// The transaction outputs
    pub outputs: Vec<BitcoinTransactionOutput>,
    /// The lock time (4 bytes)
    pub lock_time: u32,
    /// An optional 2 bytes to indicate SegWit transactions
    pub segwit_flag: bool,
}

impl<N: BitcoinNetwork> BitcoinTransactionParameters<N> {
    /// Returns a BitcoinTransactionParameters given the inputs and outputs
    pub fn new(
        inputs: Vec<BitcoinTransactionInput<N>>,
        outputs: Vec<BitcoinTransactionOutput>,
    ) -> Result<Self, TransactionError> {
        Ok(Self {
            version: 2,
            inputs,
            outputs,
            lock_time: 0,
            segwit_flag: false,
        })
    }

    /// Read and output the Bitcoin transaction parameters
    pub fn read<R: Read>(mut reader: R) -> Result<Self, TransactionError> {
        let mut version = [0u8; 4];
        reader.read(&mut version)?;

        let mut inputs = BitcoinVector::read(&mut reader, BitcoinTransactionInput::<N>::read)?;

        let segwit_flag = match inputs.is_empty() {
            true => {
                let mut flag = [0u8; 1];
                reader.read(&mut flag)?;
                match flag[0] {
                    1 => {
                        inputs = BitcoinVector::read(&mut reader, BitcoinTransactionInput::<N>::read)?;
                        true
                    }
                    _ => return Err(TransactionError::InvalidSegwitFlag(flag[0] as usize)),
                }
            }
            false => false,
        };

        let outputs = BitcoinVector::read(&mut reader, BitcoinTransactionOutput::read)?;

        if segwit_flag {
            for input in &mut inputs {
                let witnesses: Vec<Vec<u8>> = BitcoinVector::read(
                    &mut reader,
                    |s| {
                        let (size, witness) = BitcoinVector::read_witness(
                            s,
                            |sr| {
                                let mut byte = [0u8; 1];
                                sr.read(&mut byte)?;
                                Ok(byte[0])
                            }
                        )?;
                        Ok([variable_length_integer(size as u64)?, witness?].concat())
                    }
                )?;

                if !witnesses.is_empty() {
                    input.sighash_code = SignatureHash::from_byte(&witnesses[0][&witnesses[0].len() - 1]);
                    input.is_signed = true;
                }

                input.witnesses = witnesses;
            }
        }

        let mut lock_time = [0u8; 4];
        reader.read(&mut lock_time)?;

        let transaction_parameters = BitcoinTransactionParameters::<N> {
            version: u32::from_le_bytes(version),
            inputs,
            outputs,
            lock_time: u32::from_le_bytes(lock_time),
            segwit_flag,
        };

        Ok(transaction_parameters)
    }
}

/// Represents a Bitcoin transaction
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BitcoinTransaction<N: BitcoinNetwork> {
    /// The transaction parameters (version, inputs, outputs, lock_time, segwit_flag)
    pub parameters: BitcoinTransactionParameters<N>,
}

impl<N: BitcoinNetwork> fmt::Display for BitcoinTransaction<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.to_bytes().unwrap()))
    }
}

impl<N: BitcoinNetwork> Transaction for BitcoinTransaction<N> {
    type Address = BitcoinAddress<N>;
    type Format = BitcoinFormat;
    type PublicKey = BitcoinPublicKey<N>;
    type TransactionId = BitcoinTransactionId;
    type TransactionParameters = BitcoinTransactionParameters<N>;

    /// Returns an unsigned transaction given the transaction parameters.
    fn new(parameters: &Self::TransactionParameters) -> Result<Self, TransactionError> {
        Ok(Self {
            parameters: parameters.clone(),
        })
    }

    /// Returns a transaction given the transaction bytes.
    /// Note:: Raw transaction hex does not include enough
    fn from_bytes(transaction: &[u8]) -> Result<Self, TransactionError> {
        Ok(Self {
            parameters: Self::TransactionParameters::read(transaction)?,
        })
    }

    /// Returns the transaction in bytes.
    fn to_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        let mut transaction = self.parameters.version.to_le_bytes().to_vec();

        if self.parameters.segwit_flag {
            transaction.extend(vec![0x00, 0x01]);
        }

        transaction.extend(variable_length_integer(self.parameters.inputs.len() as u64)?);
        let mut has_witness = false;
        for input in &self.parameters.inputs {
            if !has_witness {
                has_witness = !input.witnesses.is_empty();
            }
            transaction.extend(input.serialize(!input.is_signed)?);
        }

        transaction.extend(variable_length_integer(
            self.parameters.outputs.len() as u64
        )?);
        for output in &self.parameters.outputs {
            transaction.extend(output.serialize()?);
        }

        if has_witness {
            for input in &self.parameters.inputs {
                match input.witnesses.len() {
                    0 => transaction.extend(vec![0x00]),
                    _ => {
                        transaction.extend(variable_length_integer(input.witnesses.len() as u64)?);
                        for witness in &input.witnesses {
                            transaction.extend(witness);
                        }
                    }
                };
            }
        }

        transaction.extend(&self.parameters.lock_time.to_le_bytes());

        Ok(transaction)
    }

    /// Returns the transaction id.
    fn to_transaction_id(&self) -> Result<Self::TransactionId, TransactionError> {
        let mut txid = double_sha2(&self.to_transaction_bytes_without_witness()?).to_vec();
        let mut wtxid = double_sha2(&self.to_bytes()?).to_vec();

        txid.reverse();
        wtxid.reverse();

        Ok(Self::TransactionId { txid, wtxid })
    }

    fn sign(&mut self, _signature: Vec<u8>, _recid: u8) -> Result<Vec<u8>, TransactionError> {
        panic!(
            "trait method sign() deprecated for bitcoin, use custom methods for signature\
             insertion in its own impl block instead."
        );
    }
}

impl<N: BitcoinNetwork> BitcoinTransaction<N> {
    /// Return the P2PKH hash preimage of the raw transaction.
    pub fn p2pkh_hash_preimage(
        &self,
        vin: usize,
        sighash: SignatureHash,
    ) -> Result<Vec<u8>, TransactionError> {
        let mut preimage = self.parameters.version.to_le_bytes().to_vec();
        preimage.extend(variable_length_integer(self.parameters.inputs.len() as u64)?);
        for (index, input) in self.parameters.inputs.iter().enumerate() {
            preimage.extend(input.serialize(index != vin)?);
        }
        preimage.extend(variable_length_integer(
            self.parameters.outputs.len() as u64
        )?);
        for output in &self.parameters.outputs {
            preimage.extend(output.serialize()?);
        }
        preimage.extend(&self.parameters.lock_time.to_le_bytes());
        preimage.extend(&(sighash as u32).to_le_bytes());
        Ok(preimage)
    }

    /// Return the SegWit hash preimage of the raw transaction
    /// https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki#specification
    pub fn segwit_hash_preimage(
        &self,
        vin: usize,
        sighash: SignatureHash,
    ) -> Result<Vec<u8>, TransactionError> {
        let mut prev_outputs = vec![];
        let mut prev_sequences = vec![];
        let mut outputs = vec![];

        for input in &self.parameters.inputs {
            prev_outputs.extend(&input.outpoint.reverse_transaction_id);
            prev_outputs.extend(&input.outpoint.index.to_le_bytes());
            prev_sequences.extend(&input.sequence);
        }

        for output in &self.parameters.outputs {
            outputs.extend(&output.serialize()?);
        }

        let input = &self.parameters.inputs[vin];
        let format = match &input.outpoint.address {
            Some(address) => address.format(),
            None => return Err(TransactionError::MissingOutpointAddress),
        };

        let script = match format {
            BitcoinFormat::Bech32 => match &input.outpoint.script_pub_key {
                Some(script) => script[1..].to_vec(),
                None => return Err(TransactionError::MissingOutpointScriptPublicKey),
            },
            BitcoinFormat::P2WSH => match &input.outpoint.redeem_script {
                Some(redeem_script) => redeem_script.to_vec(),
                None => return Err(TransactionError::InvalidInputs("P2WSH".into())),
            },
            BitcoinFormat::P2SH_P2WPKH => match &input.outpoint.redeem_script {
                Some(redeem_script) => redeem_script[1..].to_vec(),
                None => return Err(TransactionError::InvalidInputs("P2SH_P2WPKH".into())),
            },
            BitcoinFormat::P2PKH => {
                return Err(TransactionError::UnsupportedPreimage("P2PKH".into()))
            }
        };

        let mut script_code = vec![];
        if format == BitcoinFormat::P2WSH {
            script_code.extend(script);
        } else {
            script_code.push(Opcode::OP_DUP as u8);
            script_code.push(Opcode::OP_HASH160 as u8);
            script_code.extend(script);
            script_code.push(Opcode::OP_EQUALVERIFY as u8);
            script_code.push(Opcode::OP_CHECKSIG as u8);
        }
        let script_code = [
            variable_length_integer(script_code.len() as u64)?,
            script_code,
        ]
        .concat();
        let hash_prev_outputs = double_sha2(&prev_outputs);
        let hash_sequence = double_sha2(&prev_sequences);
        let hash_outputs = double_sha2(&outputs);
        let outpoint_amount = match &input.outpoint.amount {
            Some(amount) => amount.0.to_le_bytes(),
            None => return Err(TransactionError::MissingOutpointAmount),
        };

        let mut preimage = vec![];
        preimage.extend(&self.parameters.version.to_le_bytes());
        preimage.extend(hash_prev_outputs);
        preimage.extend(hash_sequence);
        preimage.extend(&input.outpoint.reverse_transaction_id);
        preimage.extend(&input.outpoint.index.to_le_bytes());
        preimage.extend(&script_code);
        preimage.extend(&outpoint_amount);
        preimage.extend(&input.sequence);
        preimage.extend(hash_outputs);
        preimage.extend(&self.parameters.lock_time.to_le_bytes());
        preimage.extend(&(sighash as u32).to_le_bytes());

        Ok(preimage)
    }

    /// Returns the transaction with the traditional serialization (no witness).
    fn to_transaction_bytes_without_witness(&self) -> Result<Vec<u8>, TransactionError> {
        let mut transaction = self.parameters.version.to_le_bytes().to_vec();

        transaction.extend(variable_length_integer(self.parameters.inputs.len() as u64)?);
        for input in &self.parameters.inputs {
            transaction.extend(input.serialize(false)?);
        }

        transaction.extend(variable_length_integer(
            self.parameters.outputs.len() as u64
        )?);
        for output in &self.parameters.outputs {
            transaction.extend(output.serialize()?);
        }

        transaction.extend(&self.parameters.lock_time.to_le_bytes());

        Ok(transaction)
    }

    /// Update a transaction's input outpoint
    #[allow(dead_code)]
    pub fn update_outpoint(&self, outpoint: Outpoint<N>) -> Self {
        let mut new_transaction = self.clone();
        for (vin, input) in self.parameters.inputs.iter().enumerate() {
            if outpoint.reverse_transaction_id == input.outpoint.reverse_transaction_id
                && outpoint.index == input.outpoint.index
            {
                new_transaction.parameters.inputs[vin].outpoint = outpoint.clone();
            }
        }
        new_transaction
    }

    /// Insert an 'address' into the input at 'index'
    pub fn insert_address(
        &mut self,
        address: BitcoinAddress<N>,
        index: u32,
    ) -> Result<(), TransactionError> {
        self.parameters.inputs[index as usize].outpoint.address = Some(address.clone());
        self.insert_script_pub_key(create_script_pub_key(&address)?, index)
    }

    /// Insert a 'script_pub_key' into the input at 'index'
    fn insert_script_pub_key(
        &mut self,
        script: Vec<u8>,
        index: u32,
    ) -> Result<(), TransactionError> {
        self.parameters.inputs[index as usize]
            .outpoint
            .script_pub_key = Some(script);
        Ok(())
    }

    /// Insert 'signature' and 'public_key' into the 'script_sig' field of the input at
    /// 'index' to make this input signed, and returns the signed transaction stream
    pub fn sign_p2pkh(
        &mut self,
        mut signature: Vec<u8>,
        public_key: Vec<u8>,
        index: u32,
    ) -> Result<Vec<u8>, TransactionError> {
        let input = &mut self.parameters.inputs[index as usize];

        signature.push((input.sighash_code as u32).to_le_bytes()[0]);

        let signature = [variable_length_integer(signature.len() as u64)?, signature].concat();
        let public_key = [vec![public_key.len() as u8], public_key].concat();

        input.script_sig = [signature, public_key].concat();
        input.is_signed = true;

        self.to_bytes()
    }

    pub fn txid_p2pkh(&self, index: u32) -> Result<Vec<u8>, TransactionError> {
        let sighash = self.parameters.inputs[index as usize].sighash_code;
        let preimage = self.p2pkh_hash_preimage(index as usize, sighash)?;
        Ok(double_sha2(&preimage).to_vec())
    }

    pub fn get_version(&self) -> Result<u32, TransactionError> {
        Ok(self.parameters.version)
    }

    pub fn get_inputs(&self) -> Result<Vec<String>, TransactionError> {
        let mut inputs: Vec<String> = vec![];
        for input in self.parameters.inputs.iter() {
            let mut sequence: u32 = 0;
            let p: *mut u32 = &mut sequence;
            let mut p = p as *mut u8;
            unsafe {
                for i in 0..4 {
                    *p = input.sequence[i];
                    p = p.add(1);
                }
            }
            let outpoint = &input.outpoint;
            let mut txid = outpoint.reverse_transaction_id.clone();
            txid.reverse();
            let txid = hex::encode(&txid);
            let signature = hex::encode(&input.script_sig);
            let input = format!(
                "sequence: {}, txid: {}, index: {}, signature: {}, sighash: {}",
                sequence, txid, outpoint.index, signature, input.sighash_code
            );
            inputs.push(input);
        }
        Ok(inputs)
    }

    pub fn get_outputs(&self) -> Result<Vec<String>, TransactionError> {
        let mut outputs: Vec<String> = vec![];
        for output in self.parameters.outputs.iter() {
            // p2pkh script = [OP_DUP] [OP_HASH160] [pkhash_len(20)] pkhash ...
            // 'OP_DUP', 'OP_HASH160', 'pkhash_len' all occupy one byte memory
            let pkhash = &output.script_pub_key[3..23];
            let address = BitcoinAddress::<N>::from_hash160(pkhash)?;
            let output = format!("to: {}, amount: {}", address, output.amount);
            outputs.push(output);
        }
        Ok(outputs)
    }
}

impl<N: BitcoinNetwork> FromStr for BitcoinTransaction<N> {
    type Err = TransactionError;

    fn from_str(transaction: &str) -> Result<Self, Self::Err> {
        Self::from_bytes(&hex::decode(transaction)?)
    }
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use anychain_core::Transaction;

    use crate::amount::BitcoinAmount;
    use crate::Mainnet;

    use super::variable_length_integer;
    use super::BitcoinTransaction;
    use super::BitcoinTransactionInput;
    use super::BitcoinTransactionOutput;
    use super::BitcoinTransactionParameters;
    use super::Opcode;
    use super::Outpoint;
    use super::SignatureHash;
    use anychain_core::libsecp256k1::{sign, Message, SecretKey};

    fn output(address: [u8; 20], amount: i64) -> BitcoinTransactionOutput {
        BitcoinTransactionOutput {
            amount: BitcoinAmount(amount),
            script_pub_key: script_public_key(address),
        }
    }

    fn input(
        txid: Vec<u8>,
        index: u32,
        address: [u8; 20],
        amount: i64,
    ) -> BitcoinTransactionInput<Mainnet> {
        let mut reverse_transaction_id = txid;
        reverse_transaction_id.reverse();

        let outpoint = Outpoint::<Mainnet> {
            reverse_transaction_id,
            index,
            amount: Some(BitcoinAmount(amount)),
            script_pub_key: Some(script_public_key(address)),
            redeem_script: None,
            address: None,
        };

        BitcoinTransactionInput {
            outpoint,
            script_sig: vec![],
            sequence: BitcoinTransactionInput::<Mainnet>::DEFAULT_SEQUENCE.to_vec(),
            sighash_code: SignatureHash::SIGHASH_ALL_SIGHASH_FORKID,
            witnesses: vec![],
            is_signed: false,
            additional_witness: None,
            witness_script_data: None,
        }
    }

    fn script_public_key(hash: [u8; 20]) -> Vec<u8> {
        let mut script = vec![];
        script.push(Opcode::OP_DUP as u8);
        script.push(Opcode::OP_HASH160 as u8);
        script.extend(variable_length_integer(hash.len() as u64).unwrap());
        script.extend(hash);
        script.push(Opcode::OP_EQUALVERIFY as u8);
        script.push(Opcode::OP_CHECKSIG as u8);
        script
    }

    #[test]
    fn f() {
        let prev_txid = "27ce2600ed495347fce5355cf90b34f72cc9aff2b42655e1c6c995ff8afe21a0";
        let prev_txid = hex::decode(prev_txid).unwrap();

        let from = [
            3, 141, 242, 111, 126, 246, 240, 104, 89, 19, 22, 155, 205, 70, 66, 132, 101, 113, 33,
            100,
        ] as [u8; 20];

        let public_key = [
            2, 252, 28, 238, 109, 187, 243, 160, 125, 88, 121, 75, 21, 67, 192, 38, 121, 197, 170,
            229, 167, 212, 99, 22, 46, 185, 168, 111, 242, 157, 190, 62, 144,
        ]
        .to_vec();

        let to = [
            121, 176, 0, 136, 118, 38, 178, 148, 169, 20, 80, 26, 76, 210, 38, 181, 139, 35, 89,
            131,
        ] as [u8; 20];

        let input = input(prev_txid, 0, from, 10100000);
        let out = output(to, 5000000);
        let out1 = output(from, 5000000);

        let params =
            BitcoinTransactionParameters::<Mainnet>::new(vec![input], vec![out, out1]).unwrap();
        let mut tx = BitcoinTransaction::<Mainnet>::new(&params).unwrap();

        println!("raw tx = {}\n", tx);

        let hash = tx.txid_p2pkh(0).unwrap();

        let signing_key = [
            56, 127, 139, 242, 234, 208, 96, 112, 134, 251, 100, 45, 230, 217, 251, 107, 58, 234,
            218, 188, 213, 253, 10, 92, 251, 17, 190, 150, 100, 177, 1, 22,
        ] as [u8; 32];

        let signing_key = SecretKey::parse_slice(&signing_key).unwrap();
        let msg = Message::parse_slice(&hash).unwrap();

        // here we sign the hash with 'signing_key'
        let signature = sign(&msg, &signing_key).0;

        // let signature = signature.serialize().to_vec();
        let signature = signature.serialize_der().as_ref().to_vec();

        println!("len = {}", signature.len());

        let tx = tx.sign_p2pkh(signature, public_key, 0).unwrap();

        // let tx = BitcoinTransaction::<Mainnet>::from_bytes(&tx).unwrap();

        // println!("tx = {}", tx);
    }

    #[test]
    fn ff() {
        let tx = "0200000001a021fe8aff95c9c6e15526b4f2afc92cf7340bf95c35e5fc475349ed0026ce27000000006a47304402204fb4a52ed0a57c609bbc1601472df96ec155a0e43f84391405f35b9d6ba688bb02203d32f513e2d7fa76957f082230458ff650bdea4b6f095dd9126b1602556fa755412102fc1cee6dbbf3a07d58794b1543c02679c5aae5a7d463162eb9a86ff29dbe3e90ffffffff02404b4c00000000001976a91479b000887626b294a914501a4cd226b58b23598388ac404b4c00000000001976a914038df26f7ef6f0685913169bcd4642846571216488ac00000000";
        let tx = "0100000001883e3ada0cba486531b64fa0d3155490f8b0c15e58078656fb1fb3dca60fdba6010000006b483045022100f8ec42af41ce34ded28342cc4b17e34747a3193dc1df7bf051f5773781d2854a022053eaf7f084ae46db6903bca8951c3162b0ccff4fe660b767f5ee8dff7f87baf30121033ef983fea45ada66ff5bc0a43b1afb0fede399397cbc8857778dc11202a55016000000100322020000000000001976a914d6b984a50fbdb748add803edf532a4d32e49dbe488ac6f6b0b00000000001976a914a0c21e8ecfeca2fa8648b1cf1cb80402fbdad61188ac0000000000000000166a146f6d6e69000000000000001f00000011224e498000000000";
        let tx = BitcoinTransaction::<Mainnet>::from_str(tx).unwrap();

        tx.get_inputs()
            .unwrap()
            .iter()
            .for_each(|s| println!("{}", s));

        let sig = "483045022100f8ec42af41ce34ded28342cc4b17e34747a3193dc1df7bf051f5773781d2854a022053eaf7f084ae46db6903bca8951c3162b0ccff4fe660b767f5ee8dff7f87baf30121033ef983fea45ada66ff5bc0a43b1afb0fede399397cbc8857778dc11202a55016";
        println!("len = {}", sig.len());
    }
}
