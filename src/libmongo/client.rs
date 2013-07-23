/* Copyright 2013 10gen Inc.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::*;

use bson::encode::*;

use util::*;
use msg::*;
use conn::Connection;
use conn_node::NodeConnection;
use conn_replica::ReplicaSetConnection;
use db::DB;
use coll::Collection;

/**
 * User interfaces with `Client`, which processes user requests
 * and sends them through the connection.
 *
 * All communication to server goes through `Client`, i.e. `DB`,
 * `Collection`, etc. all store their associated `Client`
 */
pub struct Client {
    priv conn : ~cell::Cell<@Connection>,
    priv cur_requestId : ~cell::Cell<i32>,      // first unused requestId
    // XXX index cache?
}

impl Client {
    /**
     * Creates a new Mongo client.
     *
     * Currently can connect to single unreplicated, unsharded
     * server via `connect`.
     *
     * # Returns
     * empty `Client`
     */
    pub fn new() -> Client {
        Client {
            conn : ~cell::Cell::new_empty(),
            cur_requestId : ~cell::Cell::new(0),
        }
    }

    pub fn get_admin(@self) -> DB {
        DB::new(~"admin", self)
    }

    /**
     * Returns vector of database names.
     *
     * # Returns
     * vector of database names on success, `MongoErr` on any failure
     *
     * # Failure Types
     * * errors propagated from `run_command`
     * * response from server not in expected form (must contain
     *      "databases" field whose value is array of docs containing
     *      "name" fields of `UString`s)
     */
    pub fn get_dbs(@self) -> Result<~[~str], MongoErr> {
        let mut names : ~[~str] = ~[];

        // run_command from admin database
        let db = DB::new(~"admin", self);
        let resp = match db.run_command(SpecNotation(~"{ \"listDatabases\":1 }")) {
            Ok(doc) => doc,
            Err(e) => return Err(e),
        };

        // pull out database names
        let list = match resp.find(~"databases") {
            None => return Err(MongoErr::new(
                            ~"client::get_dbs",
                            ~"could not get databases",
                            ~"missing \"databases\" field in reply")),
            Some(tmp_doc) => {
                let tmp = copy *tmp_doc;
                match tmp {
                    Array(l) => l,
                    _ => return Err(MongoErr::new(
                            ~"client::get_dbs",
                            ~"could not get databases",
                            ~"\"databases\" field in reply not an Array")),
                }
            }
        };
        let fields = list.fields;
        for fields.iter().advance |&(_, @doc)| {
            match doc {
                Embedded(bson_doc) => match bson_doc.find(~"name") {
                    Some(tmp_doc) => {
                        match (copy *tmp_doc) {
                            UString(n) => names.push(n),
                            x => return Err(MongoErr::new(
                                        ~"client::get_dbs",
                                        ~"could not extract database name",
                                        fmt!("name field %? not UString", x))),

                        }
                    }
                    None => return Err(MongoErr::new(
                                ~"client::get_dbs",
                                ~"could not extract database name",
                                fmt!("no name field in %?", bson_doc))),

                },
                _ => return Err(MongoErr::new(
                                ~"client::get_dbs",
                                ~"could not extract database name",
                                fmt!("no BsonDocument in %?", doc))),
            }
        }

        Ok(names)
    }

    /**
     * Gets the specified `DB`.
     * Alternative to constructing the `DB` explicitly
     * (`DB::new(db, client)`).
     *
     * # Arguments
     * * `db` - name of `DB` to get
     *
     * # Returns
     * handle to specified database
     */
    pub fn get_db(@self, db : ~str) -> DB {
        DB::new(db, self)
    }

    /**
     * Drops the given database.
     *
     * # Arguments
     * * `db` - name of database to drop
     *
     * # Returns
     * () on success, MongoErr on failure
     *
     * # Failure Types
     * * anything propagated from run_command
     */
    pub fn drop_db(@self, db : ~str) -> Result<(), MongoErr> {
        let db = DB::new(db, self);
        match db.run_command(SpecNotation(~"{ \"dropDatabase\":1 }")) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /**
     * Gets the specified `Collection`.
     * Alternative to constructing the `Collection` explicitly
     * (`Collection::new(db, collection, client)`).
     *
     * # Arguments
     * * `db` - database from which to get collection
     * * `coll` - name of `Collection` to get
     *
     * # Returns
     * handle to specified collection
     */
    pub fn get_collection(@self, db : ~str, coll : ~str) -> Collection {
        Collection::new(db, coll, self)
    }

    /*
     * Helper function for connections.
     */
    pub fn _connect_to_conn(&self, call : ~str, conn : @Connection)
                -> Result<(), MongoErr> {
        // check if already connected
        if !self.conn.is_empty() {
            return Err(MongoErr::new(
                            call,
                            ~"already connected",
                            ~"cannot connect if already connected; please first disconnect"));
        }

        // otherwise, make connection and connect to it
        match conn.connect() {
            Ok(_) => {
                self.conn.put_back(conn);
                Ok(())
            }
            Err(e) => return Err(MongoErr::new(
                                    call,
                                    ~"connecting",
                                    fmt!("-->\n%s", e.to_str()))),
        }
    }

    /**
     * Connects to a single server.
     *
     * # Arguments
     * * `server_ip_str` - string containing IP address of server
     * * `server_port` - port to which to connect
     *
     * # Returns
     * () on success, `MongoErr` on failure
     *
     * # Failure Types
     * * already connected
     * * network
     */
    // XXX possibly make take enum of args for node, rs, etc.
    pub fn connect(&self, server_ip_str : ~str, server_port : uint)
                -> Result<(), MongoErr> {
        self._connect_to_conn(  ~"client::connect",
                                @NodeConnection::new(server_ip_str,
                                                        server_port)
                                    as @Connection)
    }

    /**
     * Connect to replica set with specified seed list.
     *
     * # Arguments
     * `seed` - seed list (vector) of ip/port pairs
     *
     * # Returns
     * () on success, MongoErr on failure
     */
    // TODO uri parsing
    pub fn connect_to_rs(&self, seed : ~[(~str, uint)]) -> Result<@ReplicaSetConnection, MongoErr> {
        let tmp = @ReplicaSetConnection::new(seed);
        match self._connect_to_conn(  ~"client::connect_to_rs",
                                //@ReplicaSetConnection::new(seed)
                                tmp
                                    as @Connection) {
            Ok(_) => Ok(tmp),
            Err(e) => Err(e)
        }
    }

    /**
     * Disconnect from server.
     * Simultaneously empties connection cell.
     *
     * # Returns
     * () on success, `MongoErr` on failure
     *
     * # Failure Types
     * * network
     */
    pub fn disconnect(&self) -> Result<(), MongoErr> {
        if !self.conn.is_empty() { self.conn.take().disconnect() }
        // XXX currently succeeds even if not previously connected
        //      (may or may not be desired)
        else { Ok(()) }
    }

    /**
     * Sends message on connection; if write, checks write concern,
     * and if query, picks up OP_REPLY.
     *
     * # Arguments
     * * `msg` - bytes to send
     * * `wc` - write concern (if applicable)
     * * `read` - whether read operation; whether `Client` should
     *                      expect an `OP_REPLY` from the server
     *
     * # Returns
     * if read operation, `OP_REPLY` on success, `MongoErr` on failure;
     * if write operation, `None` on no last error, `MongoErr` on last error
     *      or network error
     */
    // TODO check_primary for replication purposes?
    pub fn _send_msg(@self, msg : ~[u8],
                            wc_pair : (&~str, Option<~[WRITE_CONCERN]>),
                            read : bool)
                -> Result<Option<ServerMsg>, MongoErr> {
        // first send message, exiting if network error
        match self.send(msg, read) {
            Ok(_) => (),
            Err(e) => return Err(MongoErr::new(
                                    ~"client::_send_msg",
                                    ~"",
                                    fmt!("-->\n%s", e.to_str()))),
        }

        // handle write concern or handle query as appropriate
        if !read {
            // requested write concern
            let (db_str, wc) = wc_pair;
            let db = DB::new(copy *db_str, self);

            match db.get_last_error(wc) {
                Ok(_) => Ok(None),
                Err(e) => Err(MongoErr::new(
                                    ~"client::_send_msg",
                                    ~"write concern error",
                                    fmt!("-->\n%s", e.to_str()))),
            }
        } else {
            // requested query
            match self._recv_msg(read) {
                Ok(m) => Ok(Some(m)),
                Err(e) => Err(MongoErr::new(
                                    ~"client::_send_msg",
                                    ~"error in response",
                                    fmt!("-->\n%s", e.to_str()))),
            }
        }
    }

    /**
     * Picks up server response.
     *
     * # Returns
     * `ServerMsg` on success, `MongoErr` on failure
     *
     * # Failure Types
     * * invalid bytestring/message returned (should never happen)
     * * server returned message with error flags
     * * network
     */
    fn _recv_msg(&self, read : bool) -> Result<ServerMsg, MongoErr> {
        // receive message
        let m = match self.recv(read) {
            Ok(bytes) => match parse_reply(bytes) {
                Ok(m_tmp) => m_tmp,
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        };

        // check if any errors in response and convert to MongoErr,
        //      else pass along
//println(fmt!("received message : %?", m));
        match copy m {
            OpReply { header:_, flags:f, cursor_id:_, start:_, nret:_, docs:_ } => {
                if (f & CUR_NOT_FOUND as i32) != 0i32 {
                    return Err(MongoErr::new(
                                ~"client::_recv_msg",
                                ~"CursorNotFound",
                                ~"cursor ID not valid at server"));
                } else if (f & QUERY_FAIL as i32) != 0i32 {
                    return Err(MongoErr::new(
                                ~"client::_recv_msg",
                                ~"QueryFailure",
                                ~"tmp"));
                }
                return Ok(m)
            }
        }
    }

    /**
     * Sends on `Connection` affiliated with this `Client`.
     *
     * # Arguments
     * * `bytes` - bytes to send
     *
     * # Returns
     * () on success, `MongoErr` on failure
     *
     * # Failure Types
     * * not connected
     * * network
     */
    fn send(&self, bytes : ~[u8], read : bool) -> Result<(), MongoErr> {
        if self.conn.is_empty() {
            Err(MongoErr::new(
                    ~"client::send",
                    ~"client not connected",
                    ~"attempted to send on nonexistent connection"))
        } else {
            let tmp = self.conn.take();
            let result = tmp.send(bytes, read);
            self.conn.put_back(tmp);
            result
        }
    }

    /**
     * Receives on `Connection` affiliated with this `Client`.
     *
     * # Returns
     * bytes received over connection on success, `MongoErr` on failure
     *
     * # Failure Types
     * * not connected
     * * network
     */
    fn recv(&self, read : bool) -> Result<~[u8], MongoErr> {
        if self.conn.is_empty() {
            Err(MongoErr::new(
                    ~"client::recv",
                    ~"client not connected",
                    ~"attempted to receive on nonexistent connection"))
        } else {
            let tmp = self.conn.take();
            let result = tmp.recv(read);
            self.conn.put_back(tmp);
            result
        }
    }

    /**
     * Returns first unused requestId.
     */
    pub fn get_requestId(&self) -> i32 { self.cur_requestId.take() }

    /**
     * Increments first unused requestId and returns former value.
     */
    pub fn inc_requestId(&self) -> i32 {
        let tmp = self.cur_requestId.take();
        self.cur_requestId.put_back(tmp+1);
        tmp
    }
}
