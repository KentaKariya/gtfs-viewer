use std::collections::HashMap;
use std::error::Error;

use chrono::{Duration, NaiveDateTime};
use regex::Regex;
use rusqlite::{Connection, Result, Row};

use crate::db::types::{BoardType, Service, ServiceException, Station, Stop, Weekday};
use crate::db::util::{str_to_date, str_to_dur};

mod util;
pub mod types;

//region Queries
const SERVICE_QUERY: &str = "SELECT s.*, se.service_date, se.exception_type \
    FROM service s \
    LEFT JOIN service_exception se \
    ON se.service_id = s.service_id;";

const STOP_QUERY: &str = "SELECT \
    st.arrival_time, st.departure_time, t.trip_id, s.service_id, t.short_name, t.headsign \
    FROM stop_time st \
    INNER JOIN trip t ON t.trip_id = st.trip_id \
    INNER JOIN service s ON s.service_id = t.service_id \
    INNER JOIN route r ON r.route_id = t.route_id \
    INNER JOIN agency a ON a.agency_id = r.agency_id
    WHERE st.stop_id LIKE ?1;";

const TRIP_QUERY: &str = "SELECT
    st.arrival_time, st.departure_time, st.trip_id, 0, '', s.name \
    FROM stop_time st \
    INNER JOIN stop s on s.stop_id = st.stop_id \
    WHERE st.trip_id = ?1 \
    ORDER BY st.stop_sequence;";

fn get_station_query(input: &str) -> String {
    let filter = match input.is_empty() {
        true => String::from("'%Hbf' OR name LIKE '%Hauptbahnhof'"),
        false => format!("'%{}%'", input)
    };

    format!(
        "SELECT MIN(stop_id), name FROM stop WHERE name LIKE {} GROUP BY name;",
        filter
    )
}
//endregion

pub struct GTFSDatabase {
    db: Connection,
    services: HashMap<u16, Service>,
    time_regex: Regex,
}

impl GTFSDatabase {
    pub fn new(db_path: &str) -> Result<GTFSDatabase, Box<dyn Error>> {
        let db = Connection::open(db_path)?;
        let services = fetch_services(&db)?;
        Ok(GTFSDatabase {
            db,
            services,
            time_regex: Regex::new(r"(?P<hours>\d{1,2}):(?P<minutes>\d{2}):(?P<seconds>\d{2})")?,
        })
    }

    pub fn fetch_stations(&self, input: &str) -> Result<Vec<Station>> {
        let mut stmt = self.db.prepare(&get_station_query(input))?;
        let iter = stmt.query_map([], |row| {
            Ok(Station {
                stop_id: row.get(0)?,
                name: row.get(1)?,
            })
        })?;

        Ok(iter.map(|s| s.unwrap()).collect())
    }

    pub fn fetch_stops(
        &self, stop_id: &str, board_type: BoardType, date_time: NaiveDateTime,
    ) -> Result<Vec<Stop>, Box<dyn Error>> {
        if stop_id.is_empty() {
            Ok(Vec::new())
        } else {
            let mut stmt = self.db.prepare(STOP_QUERY)?;
            // let iter = stmt.query_map([stop_id], |row| self.map_stop(&row))?;
            let iter = stmt.query_map(
                [format!("{}%", stop_id)],
                |row| self.map_stop(&row)
            )?;
            let mut stops: Vec<Stop> = iter.map(|s| s.unwrap())
                // F0: Remove unavailable service
                .filter(|s| self.services.get(&s.service_id).unwrap().is_available(
                    &(date_time.date() - Duration::days(s.arrival_time.num_days()))
                ))
                // F1: Apply time filter
                .filter(|s| s.is_after_adjusted_time(&board_type, &date_time))
                .collect();

            stops.sort_by(|a, b| a.get_adjusted_dt(&board_type, &date_time).cmp(
                &b.get_adjusted_dt(&board_type, &date_time)));

            Ok(stops)
        }
    }

    pub fn fetch_trip(&self, trip_id: u32) -> Result<Vec<Stop>, Box<dyn Error>> {
        let mut stmt = self.db.prepare(TRIP_QUERY)?;
        let iter = stmt.query_map([trip_id], |row| self.map_stop(&row))?;

        Ok(iter.map(|s| s.unwrap()).collect())
    }

    fn map_stop(&self, row: &Row) -> Result<Stop> {
        Ok(Stop {
            arrival_time: str_to_dur(&self.time_regex, row.get(0)?).unwrap(),
            departure_time: str_to_dur(&self.time_regex, row.get(1)?).unwrap(),
            trip_id: row.get(2)?,
            short_name: row.get(4)?,
            service_id: row.get(3)?,
            head_sign: row.get(5)?,
        })
    }
}

//Called once at startup
//TODO: Consider lazy evaluation
pub fn fetch_services(db: &Connection) -> Result<HashMap<u16, Service>, Box<dyn Error>> {
    let mut stmt = db.prepare(SERVICE_QUERY)?;

    let mut rows = stmt.query([])?;
    println!("Mapping services...");

    let mut map: HashMap<u16, Service> = HashMap::new();

    while let Some(row) = rows.next()? {
        let service_id = row.get(0)?;

        // Components
        let exception = match row.get::<usize, String>(10) {
            Ok(x) => Some(
                ServiceException {
                    exception_date: str_to_date(x)?,
                    exception_type: row.get(11)?,
                }
            ),
            Err(_) => None,
        };

        if !map.contains_key(&service_id) {
            let operating_weekdays = Weekday::from_rows(
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            );

            let service = Service::new(
                str_to_date(row.get(8)?)?,
                str_to_date(row.get(9)?)?,
                operating_weekdays,
            );

            map.insert(service_id, service);
        }

        // Add exception to service exceptions if exists
        if let Some(x) = exception {
            map.get_mut(&service_id).unwrap().exceptions.push(x);
        }
    }

    Ok(map)
}
