use std::str::FromStr;
use std::sync::Arc;
use futures_util::{SinkExt, StreamExt};
use futures_util::stream::SplitStream;
use futures_util::stream::SplitSink;
use mpl_token_metadata::accounts::Metadata;
use reqwest::Client;
use tokio::sync::{mpsc, Mutex, MutexGuard, oneshot};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use solana_sdk::signature::Signature;
use solana_program::msg;
use solana_program::pubkey::Pubkey;
use solana_rpc_client::http_sender::HttpSender;
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client::rpc_sender::RpcSender;
use solana_rpc_client_api::request::RpcRequest;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream, tungstenite::protocol::Message, tungstenite};


#[derive(Deserialize, Serialize, Default, PartialEq, Eq, Debug, Clone)]
pub struct Token {
    pub mint: Option<String>,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub uri: Option<String>,
    pub tx_id: Option<String>,
    pub is_pumpfun: bool,
}

impl Token {
    pub fn new() -> Self {
        Self {
            mint: None,
            name: None,
            symbol: None,
            uri: None,
            tx_id: None,
            is_pumpfun: false,
        }
    }
}

pub struct TokenEngine {}

impl TokenEngine {
    pub async fn new() -> Self {
        Self { }
    }

    pub async fn run(
        &mut self,
        token_sender: mpsc::Sender<Token>
    ) {
        let (error_tx, mut error_rx) = mpsc::channel::<()>(1);
        let error_tx_cloned = error_tx.clone(); // Clone error_tx
        let sender_clone = token_sender.clone();
        let handler = tokio::spawn(async move {
            loop {
                match error_rx.recv().await {
                    Some(_) => {
                        // Retry
                        TokenEngine::start_engine(sender_clone.clone(), error_tx_cloned.clone()).await;
                    }
                    None => {
                        // Channel has been closed
                        break;
                    }
                }
            }
        });

        // First attempt to start the engine
        TokenEngine::start_engine(token_sender, error_tx.clone()).await;

        // Wait for the handler to finish
        let _ = handler.await;
    }


    async fn write_msg(ws_writer: Arc<Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>) {
        let raydium_program_id = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8".to_string();
        let mentions = vec![raydium_program_id.as_ref()]; // Your keys here
        let mentions_string = mentions.join("\",\"");
        let json_request = format!(r#"{{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "logsSubscribe",
            "params": [
                {{
                    "mentions": [ "{}" ]
                }},
                {{
                    "commitment": "finalized"
                }}
            ]
        }}"#, mentions_string);

        ws_writer.lock().await.send(Message::Text(json_request.clone() + "\n")).await.unwrap();

        match ws_writer.lock().await.send(Message::Text(json_request.clone() + "\n")).await {
            Ok(_) => {
                msg!("Json request successfully sent");
            }
            Err(e) => {
                msg!("Failed to send json request, the error is: {}", e);
            }
        }
    }

    async fn read_msg(
        ws_reader: Arc<Mutex<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
        token_sender: mpsc::Sender<Token>
    ) {
        tokio::spawn(async move {
            let mut locked_ws_reader = ws_reader.lock().await;
            while let Some(message) = locked_ws_reader.next().await {
                TokenEngine::process_message(message, token_sender.clone()).await;
            }
        });
    }

    async fn start_engine(
        token_sender: mpsc::Sender<Token>,
        error_tx: mpsc::Sender<()>
    ) {
        msg!("TOKEN ENGINE READY...");

        msg!("sending json_request");
        // self.write_msg().await;
        let rpc_endpoint_wss= "wss://shy-delicate-diagram.solana-mainnet.quiknode.pro/6b981c085b0c5b05322894ed43bd9dd2e9fccac4/".to_string();
        let ws_url = url::Url::parse(&rpc_endpoint_wss).unwrap();

        let (ws_stream, _response) = connect_async(ws_url).await.expect("Failed to connect web socket stream");
        let (writer, reader) = ws_stream.split();
        let ws_writer = Arc::new(Mutex::new(writer));
        let ws_reader = Arc::new(Mutex::new(reader));

        TokenEngine::write_msg(ws_writer.clone()).await;
        msg!("json_request sent");

        msg!("Receiving logs...");
        TokenEngine::read_msg(ws_reader, token_sender).await;

        // Ping task
        let ws_writer_clone = Arc::clone(&ws_writer);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let mut lock = ws_writer_clone.lock().await;
                if lock.send(Message::Ping(Vec::new())).await.is_err() {
                    msg!("WebSocket disconnected");
                    // Instead of returning error, send a message back
                    // indicating that an error has occurred
                    let _ = error_tx.send(()).await;
                    break;
                } else {
                    msg!("Websockets connected");
                }
            }
        });
    }

    pub async fn fetch_token(tx_id: &Signature) -> Option<Token> {
        let rpc_endpoint = "https://shy-delicate-diagram.solana-mainnet.quiknode.pro/6b981c085b0c5b05322894ed43bd9dd2e9fccac4/".to_string();
        let new_client = Client::new();
        let http_sender = HttpSender::new_with_client(rpc_endpoint.clone(), new_client);
        let rpc_client = RpcClient::new(rpc_endpoint);

        // let pump_fun_lp = env_var("PUMP_FUN_LP_KEY");
        let pump_fun_lp = "39azUYFWPz3VHgKCf3VChUwbpURdCHRxjWVowf5jUJjg".to_string();
        let params = json!([
            tx_id.to_string(),
            {
                "maxSupportedTransactionVersion": 0,
                "commitment": "confirmed",
            }
        ]);

        let transaction = http_sender.send(RpcRequest::GetTransaction, params).await.expect("Client Failed");

        let tx_msg = transaction
            .get("transaction").expect("No transactions")
            .get("message").expect("No Messages");

        // Get all account keys
        let account_keys: Vec<String> = tx_msg
            .get("accountKeys").expect("No accounts found")
            .as_array().expect("Cannot convert to array")
            .iter()
            .map(|key| key.as_str().unwrap().to_owned())
            .collect();

        // get instructions message instructions
        let tx_ins = tx_msg.get("instructions").expect("No instructions")
            .as_array().expect("Cannot convert to array");


        // loop instruction and find the one that has Raydium's program key
        let mut mint_key = String::new();
        for ins in tx_ins {
            let raydium_program_id = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8".to_string();
            let program_id_index = ins.get("programIdIndex").unwrap().as_u64().unwrap() as usize;
            let program_id = account_keys[program_id_index].clone();

            if raydium_program_id == program_id {
                let (key_a_index, key_b_index) = (8usize, 9usize);
                let ins_accounts: Vec<u64> = ins.get("accounts")
                    .unwrap()
                    .as_array()
                    .expect("Cannot convert to array")
                    .iter()
                    .map(|json| json.as_u64().expect("Cannot convert to string").to_owned())
                    .collect();

                let mint_account_index = ins_accounts[key_a_index] as usize;

                let key_exist = account_keys.get(mint_account_index).is_some();

                if key_exist {
                    mint_key = account_keys.get(mint_account_index).unwrap().to_string();
                } else {
                    msg!("NO Key at index account_keys[{}], TX_MSG: {:#?}", mint_account_index, tx_msg)
                }

                if mint_key == "So11111111111111111111111111111111111111112" {
                    let mint_account_index = ins_accounts[key_b_index] as usize;
                    mint_key = account_keys.get(mint_account_index).unwrap().to_string();
                }
                break;
            }
        }

        // Set token drop data
        if !mint_key.is_empty() {
            let key = Pubkey::from_str(&*mint_key).unwrap();
            let (parsed_key,  _) = Metadata::find_pda(&key);

            //TODO fix unwrap to match
            let parsed_account_data = rpc_client.get_account_data(&parsed_key).unwrap();
            let metadata = Metadata::from_bytes(&parsed_account_data).unwrap();

            let token = Token {
                mint: Some(key.clone().to_string()),
                name: Some(metadata.name.replace("\0", "")),
                symbol: Some(metadata.symbol.replace("\0", "")),
                uri: Some(metadata.uri.replace("\0", "")),
                tx_id: Some(tx_id.to_string()),
                is_pumpfun: account_keys.contains(&pump_fun_lp),
            };

            return Some(token);
        }
        return None;
    }

    async fn process_message(message: Result<Message, tungstenite::Error>,token_sender: mpsc::Sender<Token>) {
        if let Ok(Message::Text(text)) = message {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if let Some(array) = value.get("params")
                    .and_then(|p| p.get("result"))
                    .and_then(|r| r.get("value"))
                    .and_then(|v| v.get("logs"))
                    .and_then(|logs| logs.as_array())
                {
                    if array.iter().any(|log| log.as_str().map_or(false, |l| l.contains("initialize2"))) {
                        if let Some(sig) = value.get("params")
                            .and_then(|p| p.get("result"))
                            .and_then(|r| r.get("value"))
                            .and_then(|v| v.get("signature"))
                            .and_then(|sig| sig.as_str())
                        {
                            if let Ok(signature) = Signature::from_str(sig) {
                                if let Some(token) = TokenEngine::fetch_token(&signature).await {
                                    if token_sender.send(token).await.is_err() {
                                        eprintln!("Failed to send token drop over channel");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
 }