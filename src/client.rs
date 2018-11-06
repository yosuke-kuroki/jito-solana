use cluster_info::{NodeInfo, FULLNODE_PORT_RANGE};
use netutil::bind_in_range;
use thin_client::ThinClient;

pub fn mk_client(r: &NodeInfo) -> ThinClient {
    let (_, transactions_socket) = bind_in_range(FULLNODE_PORT_RANGE).unwrap();

    ThinClient::new(r.contact_info.rpc, r.contact_info.tpu, transactions_socket)
}
