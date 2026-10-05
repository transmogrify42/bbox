# BBOX API Endpoints

Services are available via the HTTP `GET` endpoints:

|               URL                |     Description     |
|----------------------------------|---------------------|
| `/collections`                   | List of collections |
| `/collections/{name}/items`      | Collection items    |
| `/collections/{name}/items/{id}` | Single item         |
| `/wfs`                           | Web Feature Service (WFS 1.0, 1.1, 2.0), GET and POST |


## Request examples

Inspect collections:

    x-www-browser http://127.0.0.1:8080/collections

Feature requests:

    curl -s http://127.0.0.1:8080/collections/populated_places/items | jq .

    curl -s http://127.0.0.1:8080/collections/populated_places_names/items/2 | jq .

WFS requests:

    curl -s "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetCapabilities"

    curl -s "http://127.0.0.1:8080/wfs?service=WFS&version=2.0.0&request=GetFeature&typeNames=bbox:populated_places&count=5"
