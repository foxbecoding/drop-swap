#![windows_subsystem = "windows"]

mod token_engine;

use std::rc::Rc;
use slint::{ModelRc, SharedString, VecModel};
use token_engine::Token as AppToken;

use solana_program::msg;
use tokio::sync::mpsc;
use slint_generatedApp::Token as SlintToken;
slint::include_modules!();


pub async fn get_unique_slint_tokens(token_drops: Vec<SlintToken>) -> Vec<SlintToken> {

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

#[tokio::main]
async fn main() {
    let app = App::new().unwrap();
    let (token_sender, mut token_recv): (mpsc::Sender<AppToken>, mpsc::Receiver<AppToken>) = mpsc::channel(100);
    let mut app_tokens:  Vec<AppToken> = Vec::new();
    let mut slint_tokens:  Vec<SlintToken> = Vec::new();

    let ui_layer = app.as_weak();
    let _res = slint::spawn_local(async move {
        while let Some(token) = token_recv.recv().await {

            let token_clone = token.clone();
            if !app_tokens.contains(&token) {
                let slint_token = SlintToken {
                    mint: SharedString::from(token_clone.mint.unwrap()),
                    name: SharedString::from(token_clone.name.unwrap()),
                    symbol: SharedString::from(token_clone.symbol.unwrap()),
                    tx_id: SharedString::from(token_clone.tx_id.unwrap()),
                    is_pumpfun: token.clone().is_pumpfun,
                };

                app_tokens.push(token);
                slint_tokens.push(slint_token);

                let mut unique_slint_tokens: Vec<SlintToken> = get_unique_slint_tokens(slint_tokens.clone()).await;
                unique_slint_tokens.reverse();

                let slint_token_model: Rc<VecModel<SlintToken>> = Rc::new(VecModel::from(unique_slint_tokens));
                let slint_token_model_rc = ModelRc::from(slint_token_model.clone());
                ui_layer.unwrap().set_tokens(slint_token_model_rc);

                msg!("TOKEN DROP DONE");
                msg!("NEXT DROP...");
            }
        }
    });

    
    slint::spawn_local(async move {
        let mut token_engine = token_engine::TokenEngine::new().await;
        token_engine.run(token_sender).await;
    }).expect("TODO: panic message");

    let ui_layer2 = app.as_weak();
    app.on_open_link(move || {
        let app = ui_layer2.unwrap();
        let link = app.get_url_link();
        open::that(link.to_string()).unwrap();
    });
    app.run().unwrap();
}
