// #![windows_subsystem = "windows"]

use futures_util::{SinkExt, StreamExt};
use std::env;
use std::rc::Rc;
use dotenv::dotenv;
use std::str::FromStr;
use std::sync::Arc;
use reqwest::Client;
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, MaybeTlsStream, tungstenite, tungstenite::protocol::Message, WebSocketStream};
use solana_sdk::msg;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{
    // Keypair,
    Signature
};
use solana_rpc_client::{
    http_sender::HttpSender,
    rpc_sender::RpcSender,
    rpc_client::RpcClient,
};
use solana_rpc_client_api::request::RpcRequest;

use slint::{SharedString, ModelRc, VecModel, Weak};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

use slint_generatedApp::TokenDrop as SlintTokenDrop;
use mpl_token_metadata::accounts::Metadata;

slint::include_modules!();


// fn env_var(var: &str) -> String {
//     env::var(&var).expect(&format!("{} is not set", var))
// }

fn get_token_metadata(mint: &str) -> Metadata {
    let key = Pubkey::from_str(mint).unwrap();
    let rpc_endpoint = "https://shy-delicate-diagram.solana-mainnet.quiknode.pro/6b981c085b0c5b05322894ed43bd9dd2e9fccac4/".to_string();
    let client = RpcClient::new(rpc_endpoint);

    let (parsed_key,  _) = Metadata::find_pda(&key);
    //TODO fix unwrap to match
    let parsed_account_data = client.get_account_data(&parsed_key).unwrap();
    Metadata::from_bytes(&parsed_account_data).unwrap()
}


// Method to fetch raydium accounts
pub async fn fetch_raydium_accounts(tx_id: &Signature, http_sender: &HttpSender) -> Option<SlintTokenDrop> {
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
    let mut mint_key = "".to_string();
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
            mint_key = account_keys[mint_account_index].clone();
            if mint_key == "So11111111111111111111111111111111111111112" {
                let mint_account_index = ins_accounts[key_b_index] as usize;
                mint_key = account_keys[mint_account_index].clone();
            }
            break;
        }
    }

    // Set token drop data
    if !mint_key.is_empty() {
        let account = mint_key.as_str();
        let token_metadata = get_token_metadata(account);
        let token_drop = SlintTokenDrop {
            mint: SharedString::from(account),
            solscan: SharedString::from(format!("https://solscan.io/tx/{}", tx_id).to_string()),
            birdeye: SharedString::from(format!("https://birdeye.so/token/{}?chain=solana", account)),
            dexscreener: SharedString::from(format!("https://dexscreener.com/solana/{}", account)),
            raydium: SharedString::from( format!("https://raydium.io/swap/?inputMint=sol&outputMint={}", account)),
            jupiter: SharedString::from( format!("https://jup.ag/swap/SOL-{}", account)),
            name: SharedString::from(token_metadata.name.replace("\0", "")),
            symbol: SharedString::from(token_metadata.symbol.replace("\0", "")),
            uri: SharedString::from(token_metadata.uri.replace("\0", "")),
            is_pumpfun: account_keys.contains(&pump_fun_lp)
        };

        return Some(token_drop);
    }
    return None;
}

pub struct PumpFunWatcher {
    token_drops_sender: Option<mpsc::Sender<SlintTokenDrop>>,
    pub token_drops_recv: Option<mpsc::Receiver<SlintTokenDrop>>,
    is_running: Arc<Mutex<bool>>,
    pub token_drops: Arc<Mutex<Vec<SlintTokenDrop>>>,
    tx_stream: Option<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
    pub app: App,
    pub weak_app: Weak<App>,
}

impl PumpFunWatcher{
    pub fn new() -> Self {
        let app = App::new().unwrap();
        let weak_app = app.as_weak();
        Self {
            token_drops_sender: None,
            token_drops_recv: None,
            is_running: Arc::new(Mutex::new(false)),
            token_drops: Arc::new(Mutex::new(Vec::new())),
            tx_stream: None,
            app,
            weak_app
        }
    }

    pub async fn start(&mut self) {
        dotenv().ok();
        msg!("Monitoring: Raydium Logs");


        let (token_drops_sender, mut token_drops_receiver) = mpsc::channel(100);
        self.token_drops_sender = Some(token_drops_sender.clone());
        // self.token_drops_recv = Some(token_drops_receiver);

        let token_drops_arc = Arc::clone(&self.token_drops);

        let weak_app = self.weak_app.clone();
        let _res = slint::spawn_local(async move {
            while let Some(token_drop) = token_drops_receiver.recv().await {
                let weak_app = weak_app.unwrap();

                token_drops_arc.lock().await.push(token_drop);
                let token_drops = token_drops_arc.lock().await.clone();
                // Todo check prevent duplicates
                let mut unique_token_drops = get_unique_token_drops(token_drops).await;
                unique_token_drops.reverse();
                // msg!("DROPPER:  {:#?}", token_drops.clone().len());
                let the_model : Rc<VecModel<SlintTokenDrop>> = Rc::new(VecModel::from(unique_token_drops));
                let the_model_rc = ModelRc::from(the_model.clone());
                weak_app.set_token_drops(the_model_rc);
            }
        });


        // let raydium_public_key = env_var("RAYDIUM_LP_V4_PUBLIC_KEY");
        let raydium_public_key = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8".to_string();

        // Setup Clients
        // let _rpc_endpoint = env_var("RPC_ENDPOINT");
        let _rpc_endpoint = "https://shy-delicate-diagram.solana-mainnet.quiknode.pro/6b981c085b0c5b05322894ed43bd9dd2e9fccac4/".to_string();
        // let rpc_endpoint_wss = env_var("RPC_ENDPOINT_WSS");
        let rpc_endpoint_wss = "wss://shy-delicate-diagram.solana-mainnet.quiknode.pro/6b981c085b0c5b05322894ed43bd9dd2e9fccac4/".to_string();
        let _new_client = Client::new();

        let ws_url = url::Url::parse(&rpc_endpoint_wss).unwrap();
        // Add WebSocket Stream to the PumpFunWatcher struct
        let (ws_stream, _response) = connect_async(ws_url).await.expect("Failed to connect web socket stream");
        let (mut write, read) = ws_stream.split();

        // rest of your code...
        msg!("sending json_request");

        let mentions = vec![raydium_public_key]; // Your keys here
        // let mentions = vec!["srmqPvymJeFKQ4zGQed1GFppgkRHL9kaELCbyksJtPX"]; // Your keys here
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

        write.send(Message::Text(json_request.clone() + "\n")).await.unwrap();

        match write.send(Message::Text(json_request.clone() + "\n")).await {
            Ok(_) => {
                msg!("Json request successfully sent");
            }
            Err(e) => {
                msg!("Failed to send json request, the error is: {}", e);
            }
        }
        // self.lock().await.tx_stream = Some(write);
        self.tx_stream = Some(write);


        msg!("json_request sent");

        msg!("Receiving...");
        tokio::spawn(async move {
            read.for_each(move |message| {
                // let rpc_endpoint = env_var("RPC_ENDPOINT");
                let rpc_endpoint = "https://shy-delicate-diagram.solana-mainnet.quiknode.pro/6b981c085b0c5b05322894ed43bd9dd2e9fccac4/".to_string();
                let new_client = Client::new();
                let http_sender = HttpSender::new_with_client(rpc_endpoint, new_client);
                process_message(message, token_drops_sender.clone(), http_sender)
            }).await;
        });
    }

    pub async fn stop(&mut self) {
        let is_running = *self.is_running.lock().await;
        if is_running {
            *self.is_running.lock().await = false;
            if let Some(mut tx_stream) = self.tx_stream.take() {
                let _ = tx_stream.send(Message::Close(None)).await;
            }
        }
    }
}

pub async fn get_unique_token_drops(token_drops: Vec<SlintTokenDrop>) -> Vec<SlintTokenDrop> {

    let mut seen:Vec<String> = Vec::new();
    let mut unique_token_drops = Vec::new();


    for token_drop in token_drops {
        if !seen.contains(&token_drop.mint.to_string()) {
            seen.push(token_drop.mint.to_string());
            unique_token_drops.push(token_drop.clone());
        }
    }

    unique_token_drops
}

async fn process_message(
    message: Result<Message, tungstenite::Error>,
    token_drops_sender: mpsc::Sender<SlintTokenDrop>,
    http_sender: HttpSender,
) {
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
                            if let Some(token_drop) = fetch_raydium_accounts(&signature, &http_sender).await {
                                if token_drops_sender.send(token_drop).await.is_err() {
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


#[tokio::main]
async fn main() {
    // let rpc_endpoint = "https://shy-delicate-diagram.solana-mainnet.quiknode.pro/6b981c085b0c5b05322894ed43bd9dd2e9fccac4/".to_string();
    // // let rpc_endpoint_wss = env_var("RPC_ENDPOINT_WSS");
    // let rpc_endpoint_wss = "wss://shy-delicate-diagram.solana-mainnet.quiknode.pro/6b981c085b0c5b05322894ed43bd9dd2e9fccac4/".to_string();
    // let new_client = Client::new();
    //
    //
    // let http_sender = HttpSender::new_with_client(rpc_endpoint, new_client);
    //
    // let sig = Signature::from_str("23MEk1swWhJXSsqfHYXFMJR2xKQhzMwVhvA1GjBKF1UVoFQByw33Eb2fbCtsRpz4qNuDxsKb2Wx6trcbmiKJgxH1");
    // fetch_raydium_accounts(&sig.unwrap(), &http_sender).await;

    let mut watcher = PumpFunWatcher::new();
    watcher.start().await;
    watcher.app.on_open_link(move || {
        let watcher = watcher.weak_app.unwrap();
        let link = watcher.get_url_link();
        open::that(link.to_string()).unwrap();
    });
    watcher.app.run().unwrap();
}