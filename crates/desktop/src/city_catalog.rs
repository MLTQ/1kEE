use crate::model::GeoPoint;

pub struct CityEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub country: &'static str,
    pub ascii_name: &'static str,
    pub location: GeoPoint,
    pub population: u32,
    pub aliases: &'static [&'static str],
}

pub fn all() -> &'static [CityEntry] {
    CITIES
}

pub fn by_id(id: &str) -> Option<&'static CityEntry> {
    CITIES.iter().find(|city| city.id == id)
}

pub fn search(query: &str, limit: usize) -> Vec<&'static CityEntry> {
    let trimmed = query.trim();
    let mut matches: Vec<_> = if trimmed.is_empty() {
        CITIES.iter().collect()
    } else {
        let query = trimmed.to_ascii_lowercase();
        CITIES
            .iter()
            .filter(|city| city_matches(city, &query))
            .collect()
    };

    matches.sort_by(|left, right| city_rank(left, trimmed).cmp(&city_rank(right, trimmed)));
    matches.truncate(limit);
    matches
}

fn city_matches(city: &CityEntry, query: &str) -> bool {
    let q = query.to_ascii_lowercase();
    city.name.to_ascii_lowercase().contains(&q)
        || city.ascii_name.to_ascii_lowercase().contains(&q)
        || city.country.to_ascii_lowercase().contains(&q)
        || city
            .aliases
            .iter()
            .any(|alias| alias.to_ascii_lowercase().contains(&q))
}

fn city_rank(city: &CityEntry, query: &str) -> (u8, u8, std::cmp::Reverse<u32>, &'static str) {
    if query.trim().is_empty() {
        return (2, 2, std::cmp::Reverse(city.population), city.name);
    }

    let q = query.to_ascii_lowercase();
    let name = city.name.to_ascii_lowercase();
    let ascii = city.ascii_name.to_ascii_lowercase();
    let country = city.country.to_ascii_lowercase();
    let alias_prefix = city
        .aliases
        .iter()
        .any(|alias| alias.to_ascii_lowercase().starts_with(&q));
    let alias_contains = city
        .aliases
        .iter()
        .any(|alias| alias.to_ascii_lowercase().contains(&q));

    let primary_rank = if name.starts_with(&q) || ascii.starts_with(&q) {
        0
    } else if alias_prefix || country.starts_with(&q) {
        1
    } else {
        2
    };

    let secondary_rank = if name.contains(&q)
        || ascii.contains(&q)
        || alias_contains
        || country.contains(&q)
    {
        0
    } else {
        1
    };

    (
        primary_rank,
        secondary_rank,
        std::cmp::Reverse(city.population),
        city.name,
    )
}

const CITIES: &[CityEntry] = &[
    city!("tokyo", "Tokyo", "Japan", 35.6764, 139.6500, 37435191, &["東京", "Tokio"]),
    city!("delhi", "Delhi", "India", 28.6139, 77.2090, 32900000, &["New Delhi", "Dilli"]),
    city!("shanghai", "Shanghai", "China", 31.2304, 121.4737, 29210000, &["上海"]),
    city!("sao-paulo", "Sao Paulo", "Brazil", -23.5505, -46.6333, 22430000, &["São Paulo"]),
    city!("mexico-city", "Mexico City", "Mexico", 19.4326, -99.1332, 21800000, &["Ciudad de Mexico", "CDMX"]),
    city!("cairo", "Cairo", "Egypt", 30.0444, 31.2357, 21480000, &["Al Qahirah"]),
    city!("mumbai", "Mumbai", "India", 19.0760, 72.8777, 21400000, &["Bombay"]),
    city!("beijing", "Beijing", "China", 39.9042, 116.4074, 18960000, &["Peking", "北京"]),
    city!("dhaka", "Dhaka", "Bangladesh", 23.8103, 90.4125, 18300000, &["Dacca"]),
    city!("osaka", "Osaka", "Japan", 34.6937, 135.5023, 19010000, &["大阪"]),
    city!("new-york", "New York City", "United States", 40.7128, -74.0060, 18800000, &["New York", "NYC"]),
    city!("karachi", "Karachi", "Pakistan", 24.8607, 67.0011, 16450000, &[]),
    city!("buenos-aires", "Buenos Aires", "Argentina", -34.6037, -58.3816, 15600000, &[]),
    city!("chongqing", "Chongqing", "China", 29.4316, 106.9123, 15200000, &["重庆"]),
    city!("istanbul", "Istanbul", "Turkey", 41.0082, 28.9784, 15190000, &["Constantinople"]),
    city!("kolkata", "Kolkata", "India", 22.5726, 88.3639, 14900000, &["Calcutta"]),
    city!("manila", "Manila", "Philippines", 14.5995, 120.9842, 14150000, &[]),
    city!("lagos", "Lagos", "Nigeria", 6.5244, 3.3792, 14000000, &[]),
    city!("rio", "Rio de Janeiro", "Brazil", -22.9068, -43.1729, 13600000, &["Rio"]),
    city!("tianjin", "Tianjin", "China", 39.3434, 117.3616, 13200000, &["天津"]),
    city!("kinshasa", "Kinshasa", "DR Congo", -4.4419, 15.2663, 13170000, &["Democratic Republic of the Congo"]),
    city!("guangzhou", "Guangzhou", "China", 23.1291, 113.2644, 13080000, &["Canton", "广州"]),
    city!("los-angeles", "Los Angeles", "United States", 34.0522, -118.2437, 12750000, &["LA"]),
    city!("moscow", "Moscow", "Russia", 55.7558, 37.6173, 12600000, &["Moskva"]),
    city!("shenzhen", "Shenzhen", "China", 22.5431, 114.0579, 12500000, &["深圳"]),
    city!("lahore", "Lahore", "Pakistan", 31.5204, 74.3587, 12300000, &[]),
    city!("bangalore", "Bengaluru", "India", 12.9716, 77.5946, 12300000, &["Bangalore"]),
    city!("paris", "Paris", "France", 48.8566, 2.3522, 11100000, &[]),
    city!("bogota", "Bogota", "Colombia", 4.7110, -74.0721, 11100000, &["Bogotá"]),
    city!("jakarta", "Jakarta", "Indonesia", -6.2088, 106.8456, 10800000, &[]),
    city!("chennai", "Chennai", "India", 13.0827, 80.2707, 10700000, &["Madras"]),
    city!("lima", "Lima", "Peru", -12.0464, -77.0428, 10700000, &[]),
    city!("bangkok", "Bangkok", "Thailand", 13.7563, 100.5018, 10600000, &["Krung Thep"]),
    city!("seoul", "Seoul", "South Korea", 37.5665, 126.9780, 9963000, &[]),
    city!("nagoya", "Nagoya", "Japan", 35.1815, 136.9066, 9500000, &[]),
    city!("hyderabad", "Hyderabad", "India", 17.3850, 78.4867, 9746000, &[]),
    city!("london", "London", "United Kingdom", 51.5074, -0.1278, 9540000, &[]),
    city!("tehran", "Tehran", "Iran", 35.6892, 51.3890, 9300000, &["Teheran"]),
    city!("chicago", "Chicago", "United States", 41.8781, -87.6298, 8610000, &[]),
    city!("chengdu", "Chengdu", "China", 30.5728, 104.0668, 8370000, &["成都"]),
    city!("nanjing", "Nanjing", "China", 32.0603, 118.7969, 8270000, &["南京"]),
    city!("wuhan", "Wuhan", "China", 30.5928, 114.3055, 8200000, &["武汉"]),
    city!("ho-chi-minh", "Ho Chi Minh City", "Vietnam", 10.8231, 106.6297, 9000000, &["Saigon"]),
    city!("hong-kong", "Hong Kong", "China", 22.3193, 114.1694, 7480000, &["香港"]),
    city!("ahmedabad", "Ahmedabad", "India", 23.0225, 72.5714, 8450000, &[]),
    city!("kuala-lumpur", "Kuala Lumpur", "Malaysia", 3.1390, 101.6869, 8200000, &["KL"]),
    city!("singapore", "Singapore", "Singapore", 1.3521, 103.8198, 5900000, &[]),
    city!("baghdad", "Baghdad", "Iraq", 33.3152, 44.3661, 8700000, &[]),
    city!("santiago", "Santiago", "Chile", -33.4489, -70.6693, 7000000, &[]),
    city!("madrid", "Madrid", "Spain", 40.4168, -3.7038, 6700000, &[]),
    city!("barcelona", "Barcelona", "Spain", 41.3874, 2.1686, 5600000, &[]),
    city!("rome", "Rome", "Italy", 41.9028, 12.4964, 4300000, &["Roma"]),
    city!("milan", "Milan", "Italy", 45.4642, 9.1900, 4200000, &["Milano"]),
    city!("berlin", "Berlin", "Germany", 52.5200, 13.4050, 3800000, &[]),
    city!("hamburg", "Hamburg", "Germany", 53.5511, 9.9937, 1800000, &[]),
    city!("amsterdam", "Amsterdam", "Netherlands", 52.3676, 4.9041, 2500000, &[]),
    city!("brussels", "Brussels", "Belgium", 50.8503, 4.3517, 2100000, &["Bruxelles"]),
    city!("vienna", "Vienna", "Austria", 48.2082, 16.3738, 2000000, &["Wien"]),
    city!("prague", "Prague", "Czech Republic", 50.0755, 14.4378, 1300000, &["Praha"]),
    city!("warsaw", "Warsaw", "Poland", 52.2297, 21.0122, 1800000, &["Warszawa"]),
    city!("budapest", "Budapest", "Hungary", 47.4979, 19.0402, 1750000, &[]),
    city!("bucharest", "Bucharest", "Romania", 44.4268, 26.1025, 1800000, &["București"]),
    city!("athens", "Athens", "Greece", 37.9838, 23.7275, 3150000, &[]),
    city!("lisbon", "Lisbon", "Portugal", 38.7223, -9.1393, 2800000, &["Lisboa"]),
    city!("stockholm", "Stockholm", "Sweden", 59.3293, 18.0686, 1700000, &[]),
    city!("oslo", "Oslo", "Norway", 59.9139, 10.7522, 1100000, &[]),
    city!("copenhagen", "Copenhagen", "Denmark", 55.6761, 12.5683, 1350000, &["Kobenhavn"]),
    city!("dublin", "Dublin", "Ireland", 53.3498, -6.2603, 1200000, &[]),
    city!("zurich", "Zurich", "Switzerland", 47.3769, 8.5417, 1500000, &["Zürich"]),
    city!("kyiv", "Kyiv", "Ukraine", 50.4501, 30.5234, 2950000, &["Kiev"]),
    city!("belgrade", "Belgrade", "Serbia", 44.7866, 20.4489, 1400000, &["Beograd"]),
    city!("sarajevo", "Sarajevo", "Bosnia and Herzegovina", 43.8563, 18.4131, 275000, &[]),
    city!("tirana", "Tirana", "Albania", 41.3275, 19.8187, 500000, &[]),
    city!("istanbul", "Istanbul", "Turkey", 41.0082, 28.9784, 15190000, &["İstanbul"]),
    city!("dubai", "Dubai", "United Arab Emirates", 25.2048, 55.2708, 3600000, &[]),
    city!("abu-dhabi", "Abu Dhabi", "United Arab Emirates", 24.4539, 54.3773, 1500000, &[]),
    city!("riyadh", "Riyadh", "Saudi Arabia", 24.7136, 46.6753, 7600000, &[]),
    city!("doha", "Doha", "Qatar", 25.2854, 51.5310, 2400000, &[]),
    city!("tel-aviv", "Tel Aviv", "Israel", 32.0853, 34.7818, 4600000, &["Tel Aviv-Yafo"]),
    city!("jerusalem", "Jerusalem", "Israel", 31.7683, 35.2137, 950000, &[]),
    city!("nairobi", "Nairobi", "Kenya", -1.2921, 36.8219, 5100000, &[]),
    city!("addis-ababa", "Addis Ababa", "Ethiopia", 8.9806, 38.7578, 5200000, &[]),
    city!("johannesburg", "Johannesburg", "South Africa", -26.2041, 28.0473, 6000000, &["Joburg"]),
    city!("cape-town", "Cape Town", "South Africa", -33.9249, 18.4241, 4600000, &[]),
    city!("lagos2", "Lagos", "Nigeria", 6.5244, 3.3792, 14000000, &[]),
    city!("casablanca", "Casablanca", "Morocco", 33.5731, -7.5898, 3400000, &[]),
    city!("algiers", "Algiers", "Algeria", 36.7538, 3.0588, 3900000, &[]),
    city!("toronto", "Toronto", "Canada", 43.6532, -79.3832, 6200000, &[]),
    city!("montreal", "Montreal", "Canada", 45.5017, -73.5673, 4300000, &["Montréal"]),
    city!("vancouver", "Vancouver", "Canada", 49.2827, -123.1207, 2600000, &[]),
    city!("seattle", "Seattle", "United States", 47.6062, -122.3321, 4100000, &[]),
    city!("san-francisco", "San Francisco", "United States", 37.7749, -122.4194, 4700000, &["SF"]),
    city!("los-angeles2", "Los Angeles", "United States", 34.0522, -118.2437, 12750000, &["LA"]),
    city!("washington", "Washington, DC", "United States", 38.9072, -77.0369, 6300000, &["Washington DC"]),
    city!("miami", "Miami", "United States", 25.7617, -80.1918, 6100000, &[]),
    city!("houston", "Houston", "United States", 29.7604, -95.3698, 7100000, &[]),
    city!("atlanta", "Atlanta", "United States", 33.7490, -84.3880, 6100000, &[]),
    city!("boston", "Boston", "United States", 42.3601, -71.0589, 4900000, &[]),
    city!("philadelphia", "Philadelphia", "United States", 39.9526, -75.1652, 5700000, &["Philly"]),
    city!("chicago2", "Chicago", "United States", 41.8781, -87.6298, 8610000, &[]),
    city!("mexico-city2", "Mexico City", "Mexico", 19.4326, -99.1332, 21800000, &["CDMX"]),
    city!("guadalajara", "Guadalajara", "Mexico", 20.6597, -103.3496, 5200000, &[]),
    city!("monterrey", "Monterrey", "Mexico", 25.6866, -100.3161, 5300000, &[]),
    city!("havana", "Havana", "Cuba", 23.1136, -82.3666, 2100000, &["La Habana"]),
    city!("caracas", "Caracas", "Venezuela", 10.4806, -66.9036, 2900000, &[]),
    city!("sydney", "Sydney", "Australia", -33.8688, 151.2093, 5300000, &[]),
    city!("melbourne", "Melbourne", "Australia", -37.8136, 144.9631, 5100000, &[]),
    city!("brisbane", "Brisbane", "Australia", -27.4698, 153.0251, 2600000, &[]),
    city!("perth", "Perth", "Australia", -31.9505, 115.8605, 2100000, &[]),
    city!("auckland", "Auckland", "New Zealand", -36.8509, 174.7645, 1700000, &[]),
];

macro_rules! city {
    ($id:expr, $name:expr, $country:expr, $lat:expr, $lon:expr, $population:expr, $aliases:expr) => {
        CityEntry {
            id: $id,
            name: $name,
            country: $country,
            ascii_name: $name,
            location: GeoPoint {
                lat: $lat,
                lon: $lon,
            },
            population: $population,
            aliases: $aliases,
        }
    };
}
