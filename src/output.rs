//! DuckDB 1.5.6 C ABI. Each scan claims a disjoint Rust slice and writes one
//! DuckDB-owned output vector. All borrowed backing data outlives the database.
use crate::{
    input::{Catalog, Str},
    sessions::{Credit, Daily, UserDay},
};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::Path,
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

type Handle = *mut c_void;
type Callback = unsafe extern "C" fn(Handle);
#[derive(Default)]
#[repr(C)]
struct QueryResult {
    columns: u64,
    rows: u64,
    changed: u64,
    old_data: Handle,
    old_error: Handle,
    data: Handle,
}
unsafe extern "C" {
    fn duckdb_open(path: *const c_char, db: *mut Handle) -> u32;
    fn duckdb_connect(db: Handle, con: *mut Handle) -> u32;
    fn duckdb_disconnect(con: *mut Handle);
    fn duckdb_close(db: *mut Handle);
    fn duckdb_query(con: Handle, sql: *const c_char, r: *mut QueryResult) -> u32;
    fn duckdb_result_error(r: *mut QueryResult) -> *const c_char;
    fn duckdb_destroy_result(r: *mut QueryResult);
    fn duckdb_create_table_function() -> Handle;
    fn duckdb_destroy_table_function(f: *mut Handle);
    fn duckdb_table_function_set_name(f: Handle, name: *const c_char);
    fn duckdb_table_function_set_extra_info(f: Handle, data: Handle, drop: Option<Callback>);
    fn duckdb_table_function_set_bind(f: Handle, cb: Callback);
    fn duckdb_table_function_set_init(f: Handle, cb: Callback);
    fn duckdb_table_function_set_function(f: Handle, cb: unsafe extern "C" fn(Handle, Handle));
    fn duckdb_register_table_function(con: Handle, f: Handle) -> u32;
    fn duckdb_bind_get_extra_info(i: Handle) -> Handle;
    fn duckdb_create_logical_type(t: u32) -> Handle;
    fn duckdb_destroy_logical_type(t: *mut Handle);
    fn duckdb_bind_add_result_column(i: Handle, name: *const c_char, t: Handle);
    fn duckdb_bind_set_cardinality(i: Handle, n: u64, exact: bool);
    fn duckdb_init_get_extra_info(i: Handle) -> Handle;
    fn duckdb_init_set_max_threads(i: Handle, n: u64);
    fn duckdb_function_get_extra_info(i: Handle) -> Handle;
    fn duckdb_vector_size() -> u64;
    fn duckdb_data_chunk_get_vector(chunk: Handle, col: u64) -> Handle;
    fn duckdb_data_chunk_set_size(chunk: Handle, n: u64);
    fn duckdb_vector_get_data(v: Handle) -> Handle;
    fn duckdb_unsafe_vector_assign_string_element_len(v: Handle, i: u64, s: *const c_char, n: u64);
    fn duckdb_vector_ensure_validity_writable(v: Handle);
    fn duckdb_vector_get_validity(v: Handle) -> *mut u64;
    fn duckdb_validity_set_row_invalid(valid: *mut u64, i: u64);
}
struct Db {
    db: Handle,
    con: Handle,
}
impl Drop for Db {
    fn drop(&mut self) {
        unsafe {
            if !self.con.is_null() {
                duckdb_disconnect(&mut self.con);
            }
            if !self.db.is_null() {
                duckdb_close(&mut self.db);
            }
        }
    }
}
impl Db {
    fn open() -> Result<Self, String> {
        unsafe {
            let mut s = Self {
                con: ptr::null_mut(),
                db: ptr::null_mut(),
            };
            if duckdb_open(ptr::null(), &mut s.db) != 0 || duckdb_connect(s.db, &mut s.con) != 0 {
                return Err("failed to initialize DuckDB".into());
            }
            Ok(s)
        }
    }
    fn query(&self, sql: String) -> Result<(), String> {
        let sql = CString::new(sql).map_err(|e| e.to_string())?;
        let mut r = QueryResult::default();
        unsafe {
            let res = if duckdb_query(self.con, sql.as_ptr(), &mut r) == 0 {
                Ok(())
            } else {
                let msg = duckdb_result_error(&mut r);
                Err(if msg.is_null() {
                    "DuckDB query failed".into()
                } else {
                    CStr::from_ptr(msg).to_string_lossy().into_owned()
                })
            };
            duckdb_destroy_result(&mut r);
            res
        }
    }
}
const TEXT: u32 = 17;
const DATE: u32 = 13;
const BIGINT: u32 = 5;
const USERS: &[(&CStr, u32)] = &[
    (c"user_id", TEXT),
    (c"day", DATE),
    (c"sessions", BIGINT),
    (c"events", BIGINT),
    (c"revenue_cents", BIGINT),
    (c"top_category", TEXT),
];
const CAMPAIGNS: &[(&CStr, u32)] = &[
    (c"campaign_id", TEXT),
    (c"channel", TEXT),
    (c"day", DATE),
    (c"sessions", BIGINT),
    (c"purchasing_sessions", BIGINT),
    (c"revenue_cents", BIGINT),
    (c"assisted_sessions", BIGINT),
];
type CampaignRow = ((u32, i32), Credit);
enum Feed<'a> {
    Users(Vec<&'a [UserDay<'a>]>),
    Campaigns(Vec<&'a [CampaignRow]>),
}
struct Source<'a> {
    feed: Feed<'a>,
    catalog: &'a Catalog<'a>,
    next: AtomicUsize,
    rows: usize,
}
impl Source<'_> {
    fn schema(&self) -> &'static [(&'static CStr, u32)] {
        match self.feed {
            Feed::Users(_) => USERS,
            Feed::Campaigns(_) => CAMPAIGNS,
        }
    }
}
unsafe extern "C" fn bind(i: Handle) {
    unsafe {
        let src = &*duckdb_bind_get_extra_info(i).cast::<Source>();
        for &(name, t) in src.schema() {
            let mut ty = duckdb_create_logical_type(t);
            duckdb_bind_add_result_column(i, name.as_ptr(), ty);
            duckdb_destroy_logical_type(&mut ty);
        }
        duckdb_bind_set_cardinality(i, src.rows as u64, true);
    }
}
unsafe extern "C" fn init(i: Handle) {
    unsafe {
        let s = &*duckdb_init_get_extra_info(i).cast::<Source>();
        let chunks = match &s.feed {
            Feed::Users(v) => v.len(),
            Feed::Campaigns(v) => v.len(),
        };
        duckdb_init_set_max_threads(i, chunks.max(1) as u64);
    }
}
unsafe extern "C" fn scan(info: Handle, chunk: Handle) {
    unsafe {
        let source = &*duckdb_function_get_extra_info(info).cast::<Source>();
        let part = source.next.fetch_add(1, Ordering::Relaxed);
        let vector = |col| duckdb_data_chunk_get_vector(chunk, col);
        let nums = |col| duckdb_vector_get_data(vector(col)).cast::<i64>();
        let day_column = |col| duckdb_vector_get_data(vector(col)).cast::<i32>();
        let text = |v, i, s: Str| {
            duckdb_unsafe_vector_assign_string_element_len(v, i, s.as_ptr().cast(), s.len() as u64)
        };
        // Column types match the bind schema. Every slice is at most vector_size,
        // and DuckDB allocates the chunk and string heap separately for each worker.
        let size = match &source.feed {
            Feed::Users(v) => v.get(part).map_or(0, |rows| {
                let (name, days, sessions, events, revenue, category) = (
                    vector(0),
                    day_column(1),
                    nums(2),
                    nums(3),
                    nums(4),
                    vector(5),
                );
                duckdb_vector_ensure_validity_writable(category);
                let mask = duckdb_vector_get_validity(category);
                for (i, r) in rows.iter().enumerate() {
                    r.name.write(|s| text(name, i as u64, s));
                    days.add(i).write(r.day);
                    sessions.add(i).write(r.sessions);
                    events.add(i).write(r.events);
                    revenue.add(i).write(r.revenue);
                    if let Some(id) = r.category {
                        text(category, i as u64, source.catalog.categories[id as usize]);
                    } else {
                        duckdb_validity_set_row_invalid(mask, i as u64);
                    }
                }
                rows.len()
            }),
            Feed::Campaigns(v) => v.get(part).map_or(0, |rows| {
                let (id, channel, days, sessions, purchasing, revenue, assists) = (
                    vector(0),
                    vector(1),
                    day_column(2),
                    nums(3),
                    nums(4),
                    nums(5),
                    nums(6),
                );
                for (i, &((c, d), r)) in rows.iter().enumerate() {
                    let (name, ch) = source.catalog.channels[c as usize];
                    text(id, i as u64, name);
                    text(channel, i as u64, ch);
                    days.add(i).write(d);
                    sessions.add(i).write(r.sessions);
                    purchasing.add(i).write(r.purchasing);
                    revenue.add(i).write(r.revenue);
                    assists.add(i).write(r.assists);
                }
                rows.len()
            }),
        };
        duckdb_data_chunk_set_size(chunk, size as u64);
    }
}
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}
pub fn write(
    output: &Path,
    mut results: Vec<Daily>,
    catalog: &Catalog,
    threads: usize,
    memory: usize,
    scratch: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let cap = unsafe { duckdb_vector_size() } as usize;
    let credits: Vec<Vec<_>> = results
        .iter_mut()
        .map(|s| s.credits.drain().collect())
        .collect();
    let users = Source {
        feed: Feed::Users(results.iter().flat_map(|s| s.users.chunks(cap)).collect()),
        catalog,
        next: 0.into(),
        rows: results.iter().map(|s| s.users.len()).sum(),
    };
    let campaigns = Source {
        feed: Feed::Campaigns(credits.iter().flat_map(|s| s.chunks(cap)).collect()),
        catalog,
        next: 0.into(),
        rows: credits.iter().map(Vec::len).sum(),
    };
    // The connection holds pointers to these stack-owned sources and is dropped first.
    let db = Db::open()?;
    db.query(format!("SET threads={threads};SET memory_limit='{}GB';SET temp_directory={};SET preserve_insertion_order=false",(memory/3).max(1),quote(&scratch.join("duckdb_tmp"))))?;
    for (table, source, name, query) in [
        ("user_daily", &users, c"user_rows", "FROM user_rows()"),
        (
            "campaign_daily",
            &campaigns,
            c"campaign_rows",
            "SELECT campaign_id, channel, day, sum(sessions)::BIGINT AS sessions, sum(purchasing_sessions)::BIGINT AS purchasing_sessions, sum(revenue_cents)::BIGINT AS revenue_cents, sum(assisted_sessions)::BIGINT AS assisted_sessions FROM campaign_rows() GROUP BY ALL",
        ),
    ] {
        unsafe {
            let mut f = duckdb_create_table_function();
            duckdb_table_function_set_name(f, name.as_ptr());
            duckdb_table_function_set_extra_info(f, ptr::from_ref(source).cast_mut().cast(), None);
            duckdb_table_function_set_bind(f, bind);
            duckdb_table_function_set_init(f, init);
            duckdb_table_function_set_function(f, scan);
            let status = duckdb_register_table_function(db.con, f);
            duckdb_destroy_table_function(&mut f);
            if status != 0 {
                return Err("failed to register DuckDB table function".into());
            }
        }
        let dir = output.join(table);
        std::fs::create_dir_all(&dir)?;
        db.query(format!(
            "COPY ({query}) TO {} (FORMAT PARQUET)",
            quote(&dir.join("part-0.parquet"))
        ))?;
    }
    Ok(())
}
