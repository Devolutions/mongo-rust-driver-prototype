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

use util::*;
use client::*;
use db::*;
//use coll::*;

use bson::encode::*;

/**
 * A shard controller. An instance of this
 * wraps a Client connection to a mongos instance.
 */
pub struct ShardController {
    mongos: @Client
}

impl ShardController {

    pub fn new(client: @Client) -> ShardController {
        //check that client points to a mongos; fail if it doesn't
        //since a new method should not return a result (I think?)
        let admin = client.get_admin();
        match admin.run_command(
            SpecNotation(~"{ 'ismaster': 1 }")) {
                Ok(res) => match res.find(~"msg") {
                    Some(&UString(~"isdbgrid")) => (),
                    _ => fail!("ShardController can only connect to a mongos instance")
                },
                _ => fail!("ShardController can only connect to a mongos instance")
            };
        ShardController { mongos: client }
    }
    /**
     * Enable sharding on the specified database.
     * The database must exist or this operation will fail.
     */
    pub fn enable_sharding(&self, db: ~str) -> Result<(), MongoErr> {
        match self.mongos.get_dbs() {
            Ok(strs) => if !(strs.contains(&db)) {
                return Err(MongoErr::new(
                    ~"shard::enable_sharding",
                    fmt!("db %s not found", db),
                    ~"sharding can only be enabled on an existing db"))
            },
            Err(e) => return Err(e)
        }

        let d = DB::new(copy db, copy self.mongos);
        match d.run_command(SpecNotation(fmt!("{ 'enableSharding': '%s' }", db))) {
            Ok(doc) => match *doc.find(~"ok").unwrap() {
                Double(1f64) => return Ok(()),
                Int32(1i32) => return Ok(()),
                Int64(1i64) => return Ok(()),
                _ => return Err(MongoErr::new(
                    ~"shard::enable_sharding",
                    fmt!("error enabling sharding on %s", db),
                    ~"the server returned ok: 0"))
            },
            Err(e) => return Err(e)
        };
    }

    /**
     * Allow this shard controller to manage a new shard.
     * Hostname can be in a variety of formats:
     * * <hostname>
     * * <hostname>:<port>
     * * <replset>/<hostname>
     * * <replset>/<hostname>:port
     */
    pub fn add_shard(&self, hostname: ~str) -> Result<(), MongoErr> {
        let admin = self.mongos.get_admin();
        match admin.run_command(SpecNotation(fmt!("{ 'addShard': '%s' }", copy hostname))) {
            Ok(doc) => match *doc.find(~"ok").unwrap() {
                Double(1f64) => return Ok(()),
                Int32(1i32) => return Ok(()),
                Int64(1i64) => return Ok(()),
                _ => return Err(MongoErr::new(
                    ~"shard::add_shard",
                    fmt!("error adding shard at %s", hostname),
                    ~"the server returned ok: 0"))
            },
            Err(e) => return Err(e)
        };
    }

    /**
     * Enable sharding on the specified collection.
     */
     pub fn shard_collection(&self, db: ~str, coll: ~str, key: QuerySpec, unique: bool) -> Result<(), MongoErr> {
        let d = DB::new(copy db, copy self.mongos);
        match d.run_command(SpecNotation(
            fmt!("{ 'shardCollection': '%s.%s', 'key': %s, 'unique': '%s' }",
                db, coll, match key {
                    SpecObj(doc) => doc.to_str(),
                    SpecNotation(ref s) => copy *s
                }, unique.to_str()))) {
            Ok(doc) => match *doc.find(~"ok").unwrap() {
                Double(1f64) => return Ok(()),
                Int32(1i32) => return Ok(()),
                Int64(1i64) => return Ok(()),
                _ => return Err(MongoErr::new(
                    ~"shard::shard_collection",
                    fmt!("error sharding collection %s.%s", db, coll),
                    ~"the server returned ok: 0"))
            },
            Err(e) => return Err(e)
        };
     }

     pub fn status(&self, verbose: bool) -> Result<~str, MongoErr> {
        let mut out = ~"";
        let config = DB::new(~"config", self.mongos);
        let version = match config.get_collection(~"version").find_one(None, None, None) {
            Ok(doc) => doc,
            Err(e) => return Err(e)
        };
        out.push_str(~"--- Sharding Status ---\n");
        out.push_str(fmt!("  sharding version: %s\n", version.to_str()));
        out.push_str(~"  shards:\n");
        match config.get_collection(~"shards").find(None, None, None) {
            Ok(ref mut c) => {
                for c.advance() |sh| {
                    out.push_str(fmt!("%s\n", sh.to_str()));
                }
            },
            Err(e) => return Err(e)
        };
        Ok(out)
     }

     ///Add a tag to the given shard.
     ///Requires MongoDB 2.2 or higher.
     pub fn add_shard_tag(&self, shard: ~str, tag: ~str) -> Result<(), MongoErr> {
        let config = DB::new(~"config", copy self.mongos);
        match config.get_collection(~"shards").find_one(
           Some(SpecNotation(fmt!("{ '_id': '%s' }", shard))), None, None) {
            Ok(_) => (),
            Err(e) => return Err(e)
        }
        match config.get_collection(~"shards").update(
            SpecNotation(fmt!("{ '_id': '%s' }", shard)),
            SpecNotation(fmt!("{ '$addToSet': { 'tags', '%s' } }", tag)),
            None, None, None) {
            Ok(_) => Ok(()),
            Err(e) => Err(e)
        }
     }
}
