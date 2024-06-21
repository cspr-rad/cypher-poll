// responsible for maintaining state
// accepts proof payloads (Routes)
// verifies proofs
mod constants;
pub mod gauth;
use axum::{routing::{get, post}, Router};
use gauth::{query_user_gpg_keys, raw_gpg_keys};
use std::{collections::HashSet, sync::Arc};
use std::env;
// registers voters / inserts new identities into the tree
// if the signature is valid
// if the account is unique
// if the public key corresponds to the associated github keys
// for the user
use client::types::IdentityPayload;
use crypto::{gpg::GpgSigner, identity::Identity};
use pgp::types::Mpi;
use risc0_prover::{prover::prove_default, verifier::verify_vote};
use voting_tree::{crypto::hash_bytes, VotingTree};
use zk_associated::storage::{InMemoryTreeState, Snapshot};

type GitHubUser = String;
#[derive(Clone)]
struct ServiceState {
    github_users: HashSet<GitHubUser>,
    tree_state: InMemoryTreeState
}
impl ServiceState {
    // register a voter, takes a risc0 receipt as input (currently not prover-generic)
    // todo: check that the GPG key is actually in the list of the GitHub User's associated Keys
    // using the GitHub API
    async fn process_registration_request(
        &mut self,
        signature: Vec<Mpi>,
        data: Vec<u8>,
        public_key: String,
        identity: Identity,
        username: GitHubUser,
    ) -> Snapshot {
        let mut signer = GpgSigner {
            secret_key_asc_path: None,
            public_key_asc_string: Some(public_key),
            signed_secret_key: None,
            signed_public_key: None,
        };
        signer.init_verifier();
        // verify that the key exists in the Username's Raw Key List
        let raw_gpg_keys: Vec<String> =
            raw_gpg_keys(&query_user_gpg_keys(env::var("GITHUB_TOKEN").unwrap()).await);
        assert!(raw_gpg_keys.contains(&signer.public_key_asc_string.clone().unwrap()));
        assert!(signer.is_valid_signature(signature, &data));
        if self.github_users.get(&username).is_some() {
            panic!("GitHubUser is not unique")
        };
        self.github_users.insert(username);
        self.tree_state.insert_nullifier(identity)
    }
}

fn default_tree_state() -> InMemoryTreeState {
    let mut voting_tree: VotingTree = VotingTree {
        zero_node: hash_bytes(vec![0; 32]),
        zero_levels: Vec::new(),
        // size must equal tree depth
        filled: vec![vec![]; 5],
        root: None,
        index: 0,
        // the maximum amount of identities this tree can store
        // is 2^depth (depth:5 => max_identity_count:32)
        depth: 5,
    };
    voting_tree.calculate_zero_levels();

    InMemoryTreeState {
        root_history: Vec::new(),
        used_nullifiers: Vec::new(),
        voting_tree,
        leafs: Vec::new(),
    }
}

// `&'static str` becomes a `200 OK` with `content-type: text/plain; charset=utf-8`
async fn ping() -> &'static str {
    "pong"
}

#[tokio::main]
async fn main() {
    print!(
        r#"
    
███████ ██   ██      ██████ ██    ██ ██████  ██   ██ ███████ ██████  
   ███  ██  ██      ██       ██  ██  ██   ██ ██   ██ ██      ██   ██ 
  ███   █████       ██        ████   ██████  ███████ █████   ██████  
 ███    ██  ██      ██         ██    ██      ██   ██ ██      ██   ██ 
███████ ██   ██      ██████    ██    ██      ██   ██ ███████ ██   ██ 
                                                                                                    

    "#
    );
    let tree_state: InMemoryTreeState =
        InMemoryTreeState::new(Vec::new(), Vec::new(), Vec::new());
    let mut service_state: ServiceState = ServiceState{
        github_users: HashSet::new(),
        tree_state
    };
    let shared_state = Arc::new(service_state);
    let app = Router::new()
    .route(
        "/ping",
        get({
            let shared_state = Arc::clone(&shared_state);
            move || ping()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[tokio::test]
async fn submit_zk_vote() {
    use crypto::identity::UniqueIdentity;
    use risc0_types::CircuitInputs;
    use risc0_zkvm::Receipt;
    use std::{collections::HashSet, fs, path::PathBuf};
    // initialize tree_state and service_state
    // process a registration request using the default keypair in ~/resources/test/
    // generate a vote proof
    // verify the vote proof and apply the vote to tree_state
    let mut tree_state: InMemoryTreeState = default_tree_state();
    let mut service_state: ServiceState = ServiceState {
        github_users: HashSet::new(),
        tree_state
    };
    let mut identity: UniqueIdentity = UniqueIdentity {
        identity: None,
        nullifier: None,
    };
    identity.generate_nullifier("I am a random seed, radnom!".to_string());

    let private_key_path_str = "/Users/chef/Desktop/cypher-poll/resources/test/key.sec.asc";
    let public_key_path_str = "/Users/chef/Desktop/cypher-poll/resources/test/key.asc";

    let public_key_string: String =
        fs::read_to_string(public_key_path_str).expect("Failed to read public key");

    let mut signer = GpgSigner {
        secret_key_asc_path: Some(PathBuf::from(private_key_path_str)),
        public_key_asc_string: Some(public_key_string.clone()),
        signed_secret_key: None,
        signed_public_key: None,
    };
    signer.init();
    let data: Vec<u8> = vec![0u8];
    let signature: Vec<Mpi> = signer.sign_bytes(&data);

    let mut serialized_signature: Vec<Vec<u8>> = Vec::new();
    for mpi in &signature {
        serialized_signature.push(mpi.as_bytes().to_vec())
    }

    let mut deserialized_signature: Vec<Mpi> = Vec::new();
    for series in &serialized_signature {
        deserialized_signature.push(Mpi::from_slice(series))
    }
    assert_eq!(&signature, &deserialized_signature);

    assert!(signer.is_valid_signature(signature.clone(), &data));
    identity.compute_public_identity(signer.signed_public_key.unwrap());
    // register the voter
    service_state
        .process_registration_request(
            signature,
            data,
            public_key_string.clone(),
            identity.identity.expect("Missing identity"),
            "jonas089".to_string(),
        )
        .await;

    // generate a proof -> redeem the nullifier
    let proof: Receipt = prove_default(CircuitInputs {
        root_history: service_state.tree_state.root_history.clone(),
        snapshot: service_state.tree_state.voting_tree.clone(),
        nullifier: identity.nullifier.clone().expect("Missing Nullifier"),
        public_key_string: public_key_string.clone(),
    });

    let is_valid: bool = verify_vote(proof, service_state.tree_state.root_history.clone());
    assert!(is_valid)
}
