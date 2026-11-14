from csvq import Table

PEOPLE_CSV = (
    "name,age,city,score\n"
    "Ada,36,London,9.5\n"
    "Bob,,Paris,7\n"
    "Cy,25,London,8.25\n"
    "Dee,41,Paris,\n"
    "Eve,25,Rome,6\n"
)


def tables():
    return {
        "people": Table.from_csv(PEOPLE_CSV),
        "one": Table(["x"], [(1,)]),
        "empty": Table(["a", "b"], []),
    }


CITIES_CSV = (
    "city,country,pop\n"
    "London,UK,9\n"
    "Paris,France,2\n"
    "Berlin,Germany,4\n"
)

COUNTRIES_CSV = (
    "country,continent\n"
    "UK,Europe\n"
    "France,Europe\n"
    "Japan,Asia\n"
)


def join_tables():
    t = tables()
    t["cities"] = Table.from_csv(CITIES_CSV)
    t["countries"] = Table.from_csv(COUNTRIES_CSV)
    return t
