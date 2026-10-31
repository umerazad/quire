enum Status {Draft='draft', Saved='saved', Failed='failed'}
export abstract class Store<K extends string, V> implements Iterable<[K,V]>{ private readonly items=new Map<K,V>(); constructor(protected readonly name:string){}
  abstract validate(value:V):value is V
  *[Symbol.iterator](){ yield* this.items } }
const s = <Store<string, number>>(null as unknown); const n = s!.name as const satisfies string