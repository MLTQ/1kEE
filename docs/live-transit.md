# Live transit

Open **Layers → Live transit** in the desktop app. Enable an agency or use **Locate** to enable it and open its service area in the local map. Agencies can run together. Boston bus reports are available through **Include buses where available**.

Circles show agency-reported vehicle coordinates. Squares mark stations named in vehicle reports or arrival predictions. Click a marker to see the line, vehicle or trip identifier, station or destination when supplied, source, and timestamp age. Trains sharing a station can be selected in the report window.

| Service | What appears on the map |
| --- | --- |
| Seattle and Tacoma | Link rail vehicles on services covered by the [Sound Transit feed](https://www.soundtransit.org/help-contacts/business-information/open-transit-data-otd/otd-downloads). |
| Boston | Subway, light rail, commuter rail, and optional buses from the [MBTA vehicle feed](https://www.mbta.com/developers/gtfs-realtime). |
| New York City | Subway reports placed at the referenced station. Status identifies trains stopped at, approaching, or in transit to that station. The app reads cached MTA data from the independent [Transiter service](https://demo.transiter.dev/systems/us-ny-subway). |
| London | One upcoming Underground arrival per identified train and line. The square marks the predicted arrival station. The detail window includes TfL’s location description when available. [Powered by TfL Open Data](https://tfl.gov.uk/info-for/open-data-users/). |
| Tokyo | Toei subway and Sakura Tram services covered by the vehicle feed. The app matches a reported stop sequence to the trip schedule to identify the station. Data from the Bureau of Transportation, Tokyo Metropolitan Government / Association for Open Data of Public Transportation. [Toei dataset, CC BY 4.0](https://ckan.odpt.org/en/dataset/r_train_gtfs_rt-odpt_train-toei). |
| Finland | National train locations, including the train number and speed when supplied. [Fintraffic / Digitraffic, CC BY 4.0](https://www.digitraffic.fi/en/railway-traffic/). |
| Malaysia | KTM rail vehicles when the operator reports an active service. [KTMB / data.gov.my, CC BY 4.0](https://developer.data.gov.my/realtime-api/gtfs-realtime). |

Enabled agencies refresh in the background every 30 seconds. Schedule and station lookups are cached for a day during the session. An empty feed can be normal outside operating hours. After a failed refresh, the agency displays its error and the previous reports continue to age. Markers dim after 90 seconds and expire after five minutes. London predictions also expire at their expected arrival time. When GTFS supplies only a feed timestamp, the detail window labels it accordingly.
